//! Listing, stat, fetch, atomic conflict-aware writes, rename, copy, the search inside files, and what an archive holds.
//!
//! Everything here is about one workspace (or one mount, which is read and listed the same way
//! and never written). A request names a path; the path is walked through [`Workspace::locate`],
//! so that it cannot leave the root; it is checked against the ignore rules both as it was spelled
//! and as it really is, so that a link cannot lead to what is left out; and the operation is then
//! done through the handle of the directory that holds the entry.
//!
//! **Writing** never changes a file in place. The content is received into a file of its own next
//! to the target, made whole and flushed, and only then renamed over the target in one step. The
//! writer says which version it saw (the hash of the content it loaded, or that there was none),
//! and the version is checked before anything is received and again just before the rename; a
//! write over a newer version is refused and leaves the file as it was. A write that is cut off at
//! any point leaves the old file whole, and the half-received file is removed.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::sync::Arc;

use chrono::DateTime;
use orch_api_contract::workspace::{DirEntry, DirEntryKind, FileStat};
use orch_link_contract::commands::{ExpectedHash, FileVersion, FsFetchResult};
use orch_types::{Hash, Time};

use crate::blobs::{
    BlobError, BlobSink, BlobSource, FileReader, FileWriter, Uploads, download_checked, hash_file,
    hash_file_prefix,
};
use crate::environment::{GateState, Limits};
use crate::error::{HostError, Result, blocking};
use crate::ignore::IgnoreRules;
use crate::index::HashCache;
use crate::media::{self, SNIFF_BYTES};
use crate::paths::{Locate, Located, RelPath, TEMP_PREFIX, Workspace, is_internal_name};
use crate::platform::{
    Dir, Kind, Meta, file_meta, is_cross_device, read_at, set_file_mode, write_at,
};
use crate::walk::WALK_DEPTH_MAX;

/// How many bytes one step of a copy moves: a mebibyte keeps the number of system calls low and
/// the time a cancelled copy keeps running short.
const COPY_CHUNK_BYTES: usize = 1024 * 1024;

/// The permission bits that Orch will set on a file it writes. The set-user-id, set-group-id and
/// sticky bits are never set from outside: a connector may run as a privileged user.
const WRITABLE_MODE_BITS: u32 = 0o777;

/// The bits a new file is made with before the process's umask takes its part.
const NEW_FILE_MODE: u32 = 0o666;

/// The bits a new directory is made with before the process's umask takes its part.
const NEW_DIR_MODE: u32 = 0o777;

/// The bits a directory has while its content is being copied into it, so that it can be written
/// whatever the original's bits are; the original's are set when the copy is done.
const COPYING_DIR_MODE: u32 = 0o700;

/// A directory as a listing returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    /// The entries: directories first, then by name without regard to case.
    pub entries: Vec<DirEntry>,
    /// Whether there are more than a listing returns.
    pub truncated: bool,
}

/// A directory's actual identity, independent of child membership and modification time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectoryLocation {
    /// Canonical workspace-relative directory, after the ordinary path/ignore checks.
    pub(crate) path: RelPath,
    device: u64,
    node: u64,
}

/// Directory identity and stat evidence collected around a streamed metadata observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectoryVersion {
    pub(crate) location: DirectoryLocation,
    identity: crate::platform::Identity,
}

/// A file opened for reading, checked to be a regular file, with the version it is.
#[derive(Debug, Clone)]
pub struct Opened {
    /// The open file.
    pub file: Arc<File>,
    /// What it was when it was opened.
    pub meta: Meta,
    /// Where it really is.
    pub real: RelPath,
}

/// The files of one workspace or one mount.
#[derive(Debug)]
pub struct Files {
    inner: Arc<Inner>,
    recorded_operation: Option<Arc<crate::state_store::DestructiveLease>>,
}

impl Clone for Files {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            recorded_operation: self.recorded_operation.clone(),
        }
    }
}

#[derive(Debug)]
struct Inner {
    workspace: Workspace,
    ignore: Option<Arc<IgnoreRules>>,
    cache: Arc<HashCache>,
    limits: Limits,
    gates: Arc<GateState>,
    uploads: Uploads,
    writable: bool,
}

fn time_of(nanos: i64) -> Time {
    Time::new(DateTime::from_timestamp_nanos(nanos))
}

fn kind_of(kind: Kind) -> DirEntryKind {
    match kind {
        Kind::File => DirEntryKind::File,
        Kind::Dir => DirEntryKind::Dir,
        Kind::Symlink => DirEntryKind::Symlink,
        Kind::Other => DirEntryKind::Other,
    }
}

/// The size a listing shows: that of a file, or of the file a link leads to. A directory has none
/// worth showing, and what a system says of the size of something else is not a size of content.
fn size_of(meta: &Meta) -> i64 {
    if meta.is_file() || meta.is_symlink() {
        i64::try_from(meta.size).unwrap_or(i64::MAX)
    } else {
        0
    }
}

fn blob_failure(error: BlobError) -> HostError {
    match error {
        BlobError::HashMismatch => {
            HostError::Changed("the content was another by the time it was sent".to_owned())
        }
        other => HostError::Content(other.to_string()),
    }
}

fn temp_name() -> OsString {
    OsString::from(format!("{TEMP_PREFIX}{}", orch_types::new_id().simple()))
}

/// Removes a half-made file or directory when the work that made it ends any other way than by
/// renaming it into place.
struct Cleanup {
    parent: Dir,
    name: OsString,
    is_dir: bool,
    armed: bool,
}

impl Cleanup {
    fn new(parent: &Dir, name: &OsStr, is_dir: bool) -> Result<Self> {
        let parent = parent
            .try_clone()
            .map_err(|error| HostError::io("holding a directory", error))?;
        Ok(Self {
            parent,
            name: name.to_owned(),
            is_dir,
            armed: true,
        })
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if self.is_dir {
            let _ = remove_tree(&self.parent, &self.name, 0);
        } else {
            let _ = self.parent.unlink(&self.name);
        }
    }
}

/// Removes the entry `name` of `parent`: a file or a link at once, a directory with all that is in
/// it, never following a link.
pub(crate) fn remove_tree(parent: &Dir, name: &OsStr, depth: usize) -> Result<()> {
    let meta = parent
        .lstat(name)
        .map_err(|error| HostError::io("looking at what is to be removed", error))?;
    if !meta.is_dir() {
        return parent
            .unlink(name)
            .map_err(|error| HostError::io("removing a file", error));
    }
    if depth >= WALK_DEPTH_MAX {
        return Err(HostError::Unsupported(
            "a directory tree this deep is not removed in one step".to_owned(),
        ));
    }
    let child = parent
        .open_child(name)
        .map_err(|error| HostError::io("entering a directory to remove it", error))?;
    for entry in child
        .names()
        .map_err(|error| HostError::io("listing a directory to remove it", error))?
    {
        remove_tree(&child, &entry, depth + 1)?;
    }
    parent
        .rmdir(name)
        .map_err(|error| HostError::io("removing a directory", error))
}

impl Files {
    /// The files of a workspace, whose ignore rules are `ignore`, written only as `gates` allow.
    #[must_use]
    pub fn workspace(
        workspace: Workspace,
        ignore: Arc<IgnoreRules>,
        cache: Arc<HashCache>,
        limits: Limits,
        gates: Arc<GateState>,
    ) -> Self {
        Self::build(workspace, Some(ignore), cache, limits, gates, true)
    }

    /// A closed owned-data root is copied whole; project ignore files cannot hide its contents.
    pub(crate) fn owned_data(
        workspace: Workspace,
        cache: Arc<HashCache>,
        limits: Limits,
        gates: Arc<GateState>,
    ) -> Self {
        Self::build(workspace, None, cache, limits, gates, true)
    }

    /// Blocking materialization clones retain destructive admission even if their caller is dropped.
    pub(crate) fn with_recorded_operation(
        mut self,
        lease: Arc<crate::state_store::DestructiveLease>,
    ) -> Self {
        self.recorded_operation = Some(lease);
        self
    }

    /// The files of a mount: listed and read as they are, with no ignore rules, and never written.
    #[must_use]
    pub fn mount(
        workspace: Workspace,
        cache: Arc<HashCache>,
        limits: Limits,
        gates: Arc<GateState>,
    ) -> Self {
        Self::build(workspace, None, cache, limits, gates, false)
    }

    fn build(
        workspace: Workspace,
        ignore: Option<Arc<IgnoreRules>>,
        cache: Arc<HashCache>,
        limits: Limits,
        gates: Arc<GateState>,
        writable: bool,
    ) -> Self {
        Self {
            recorded_operation: None,
            inner: Arc::new(Inner {
                workspace,
                ignore,
                cache,
                limits,
                gates,
                uploads: Uploads::new(),
                writable,
            }),
        }
    }

    /// The root and the rules for walking from it.
    #[must_use]
    pub fn root(&self) -> &Workspace {
        &self.inner.workspace
    }

    /// The ignore rules, where there are any.
    #[must_use]
    pub fn ignore(&self) -> Option<&Arc<IgnoreRules>> {
        self.inner.ignore.as_ref()
    }

    /// The hashes remembered for the files of this workspace.
    #[must_use]
    pub fn hashes(&self) -> &Arc<HashCache> {
        &self.inner.cache
    }

    /// The limits in force.
    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.inner.limits
    }

    /// What the machine's owner lets work from outside do here.
    #[must_use]
    pub fn gates(&self) -> &Arc<GateState> {
        &self.inner.gates
    }

    /// Whether this is a workspace, which Orch writes, and not a mount.
    #[must_use]
    pub fn is_writable(&self) -> bool {
        self.inner.writable
    }

    fn ws(&self) -> &Workspace {
        &self.inner.workspace
    }

    /// Parses a path as the environment writes it.
    pub fn parse(&self, text: &str) -> Result<RelPath> {
        self.ws().parse(text)
    }

    /// Refuses unless Orch may change files here.
    fn require_write(&self) -> Result<()> {
        if !self.inner.writable {
            return Err(HostError::Unsupported(
                "an attached mount is not written through Orch".to_owned(),
            ));
        }
        self.inner.gates.require_remote_write()
    }

    /// Refuses a path the ignore rules leave out, as it was spelled and as it really is.
    pub(crate) fn check_visible(
        &self,
        asked: &RelPath,
        located: &Located,
        is_dir: bool,
    ) -> Result<()> {
        if asked.has_component(orch_agent_contract::session::INSTRUCTION_ROOT_MARKER)
            || located
                .real
                .has_component(orch_agent_contract::session::INSTRUCTION_ROOT_MARKER)
        {
            return Err(HostError::Ignored(
                "instruction-root control metadata is not workspace content".to_owned(),
            ));
        }
        let Some(rules) = &self.inner.ignore else {
            return Ok(());
        };
        if rules.is_ignored(self.ws(), asked, is_dir)
            || rules.is_ignored(self.ws(), &located.real, is_dir)
        {
            return Err(HostError::Ignored(format!(
                "{asked} is left out by the workspace's ignore rules"
            )));
        }
        Ok(())
    }

    fn entry_of(
        &self,
        path: &RelPath,
        name: &str,
        meta: &Meta,
        link_target: Option<String>,
    ) -> DirEntry {
        DirEntry {
            name: name.to_owned(),
            path: path.to_string(),
            kind: kind_of(meta.kind),
            size: size_of(meta),
            mtime: time_of(meta.mtime_ns),
            mode: meta.mode,
            link_target,
            cached: false,
        }
    }

    /// What a link shows of itself: the version of the file it leads to when it leads to a regular
    /// file inside the workspace, since that is the version a read through it will be of.
    fn displayed_meta(&self, path: &RelPath, meta: &Meta) -> Meta {
        if !meta.is_symlink() {
            return *meta;
        }
        match self.ws().locate(path, Locate::THROUGH) {
            // What a link shows of the file it leads to is shown only if that file is not left out.
            Ok(target) if self.check_visible(path, &target, false).is_ok() => match target.meta {
                Some(found) if found.is_file() => Meta {
                    kind: Kind::Symlink,
                    ..found
                },
                _ => Meta { size: 0, ..*meta },
            },
            _ => Meta { size: 0, ..*meta },
        }
    }

    // ------------------------------------------------------------------ listing and stat

    /// Validates the same path/visibility as listing before its watch is installed.
    pub(crate) fn directory_location(&self, path: &RelPath) -> Result<DirectoryLocation> {
        let located = self.ws().locate(path, Locate::THROUGH)?;
        let meta = located
            .meta
            .ok_or_else(|| HostError::NotFound(format!("{path} is not there")))?;
        if !meta.is_dir() {
            return Err(HostError::NotFound(format!("{path} is not a directory")));
        }
        self.check_visible(path, &located, true)?;
        Ok(DirectoryLocation {
            path: located.real,
            device: meta.dev,
            node: meta.ino,
        })
    }

    /// Reads directory identity/stat evidence without enumerating children; equality is not proof
    /// that no native membership mutation occurred.
    pub(crate) fn directory_version(&self, path: &RelPath) -> Result<DirectoryVersion> {
        let located = self.ws().locate(path, Locate::THROUGH)?;
        let meta = located
            .meta
            .ok_or_else(|| HostError::NotFound(format!("{path} is not there")))?;
        if !meta.is_dir() {
            return Err(HostError::NotFound(format!("{path} is not a directory")));
        }
        self.check_visible(path, &located, true)?;
        Ok(DirectoryVersion {
            location: DirectoryLocation {
                path: located.real,
                device: meta.dev,
                node: meta.ino,
            },
            identity: meta.identity(),
        })
    }

    /// Streams visible children; a failed/replaced entry never becomes a successful omitted row.
    pub(crate) fn visit_directory(
        &self,
        path: &RelPath,
        visit: &mut dyn FnMut(DirEntry) -> Result<()>,
    ) -> Result<DirectoryVersion> {
        let before = self.directory_version(path)?;
        let located = self.ws().locate(path, Locate::THROUGH)?;
        let dir = self.ws().open_dir(&located)?;
        let scope = match &self.inner.ignore {
            Some(rules) => Some(rules.scope_at(self.ws(), &located.real).ok_or_else(|| {
                HostError::Ignored(format!(
                    "{path} is left out by the workspace's ignore rules"
                ))
            })?),
            None => None,
        };
        let mut failed = None;
        dir.visit_names(&mut |name| {
            let mut one = || -> Result<()> {
                let Some(text) = name.to_str() else {
                    return Ok(());
                };
                if is_internal_name(text) {
                    return Ok(());
                }
                let shown = path.join(text)?;
                let real = located.real.join(text)?;
                let meta = dir
                    .lstat(name)
                    .map_err(|error| HostError::io(format!("describing {shown}"), error))?;
                if scope
                    .as_ref()
                    .is_some_and(|scope| scope.is_ignored(&real, meta.is_dir()))
                {
                    return Ok(());
                }
                let target = if meta.is_symlink() {
                    Some(
                        dir.read_link(name)
                            .map_err(|error| HostError::io(format!("reading link {shown}"), error))?
                            .to_string_lossy()
                            .into_owned(),
                    )
                } else {
                    None
                };
                visit(self.entry_of(&shown, text, &self.displayed_meta(&shown, &meta), target))
            };
            match one() {
                Ok(()) => true,
                Err(error) => {
                    failed = Some(error);
                    false
                }
            }
        })
        .map_err(|error| HostError::io(format!("listing {path}"), error))?;
        if let Some(error) = failed {
            return Err(error);
        }
        if self.directory_version(path)? != before {
            return Err(HostError::Changed(
                "the directory changed during its metadata capture".to_owned(),
            ));
        }
        Ok(before)
    }

    /// Lists the directory at `path` (through a link, if it is one): at most
    /// [`orch_link_contract::commands::FS_LIST_MAX_ENTRIES`], directories first and then by name without regard to case.
    pub fn list(&self, path: &RelPath) -> Result<Listing> {
        crate::directory_pages::preview(self, path)
    }

    /// Describes the entry at `path`: what a listing says of it, the hash of its content where
    /// that is known, its media type and whether it is text. A link is described by what it
    /// leads to when that is a regular file.
    pub fn stat(&self, path: &RelPath) -> Result<FileStat> {
        let located = self.ws().locate(path, Locate::ENTRY)?;
        let meta = located
            .meta
            .ok_or_else(|| HostError::NotFound(format!("{path} is not there")))?;
        self.check_visible(path, &located, meta.is_dir())?;
        let name = located
            .name()
            .ok()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if meta.is_dir() {
            let entry = self.entry_of(path, name, &meta, None);
            return Ok(FileStat {
                entry,
                hash: None,
                media_type: media::DIRECTORY.to_owned(),
                is_text: false,
            });
        }
        if meta.is_symlink() {
            let link_target = located
                .parent
                .read_link(located.name()?)
                .ok()
                .map(|target| target.to_string_lossy().into_owned());
            if let Ok(target) = self.ws().locate(path, Locate::THROUGH)
                && target.meta.is_some_and(|found| found.is_file())
                && self.check_visible(path, &target, false).is_ok()
            {
                let (found, hash, media_type, is_text) = self.read_file_facts(&target, name)?;
                let entry = self.entry_of(
                    path,
                    name,
                    &Meta {
                        kind: Kind::Symlink,
                        ..found
                    },
                    link_target,
                );
                return Ok(FileStat {
                    entry,
                    hash,
                    media_type,
                    is_text,
                });
            }
            let entry = self.entry_of(path, name, &Meta { size: 0, ..meta }, link_target);
            return Ok(FileStat {
                entry,
                hash: None,
                media_type: media::SYMLINK.to_owned(),
                is_text: false,
            });
        }
        if !meta.is_file() {
            let entry = self.entry_of(path, name, &meta, None);
            return Ok(FileStat {
                entry,
                hash: None,
                media_type: media::UNKNOWN.to_owned(),
                is_text: false,
            });
        }
        let (found, hash, media_type, is_text) = self.read_file_facts(&located, name)?;
        Ok(FileStat {
            entry: self.entry_of(path, name, &found, None),
            hash,
            media_type,
            is_text,
        })
    }

    /// The version, the known hash, the media type and the text-ness of the regular file `located`.
    fn read_file_facts(
        &self,
        located: &Located,
        name: &str,
    ) -> Result<(Meta, Option<Hash>, String, bool)> {
        let entry_name = located.name()?;
        let file = located
            .parent
            .open_read(entry_name)
            .map_err(|error| HostError::io(format!("opening {}", located.real), error))?;
        let meta = file_meta(&file)
            .map_err(|error| HostError::io(format!("looking at {}", located.real), error))?;
        let mut head = vec![0u8; SNIFF_BYTES];
        let count = read_at(&file, &mut head, 0)
            .map_err(|error| HostError::io(format!("reading {}", located.real), error))?;
        head.truncate(count);
        let hash = self.inner.cache.get(&located.real, &meta.identity());
        Ok((
            meta,
            hash,
            media::media_type(name, &head).to_owned(),
            media::is_text(&head),
        ))
    }

    // ------------------------------------------------------------------ reading

    /// Opens the regular file at `path`, through a link if it is one, and says which version it is.
    pub fn open_regular(&self, path: &RelPath) -> Result<Opened> {
        let located = self.ws().locate(path, Locate::THROUGH)?;
        let meta = located
            .meta
            .ok_or_else(|| HostError::NotFound(format!("{path} is not there")))?;
        self.check_visible(path, &located, meta.is_dir())?;
        if !meta.is_file() {
            return Err(HostError::Unsupported(format!(
                "{path} is not a regular file, and only those are read"
            )));
        }
        let file = located
            .parent
            .open_read(located.name()?)
            .map_err(|error| HostError::io(format!("opening {path}"), error))?;
        let meta =
            file_meta(&file).map_err(|error| HostError::io(format!("looking at {path}"), error))?;
        Ok(Opened {
            file: Arc::new(file),
            meta,
            real: located.real,
        })
    }

    /// The hash of an opened file: remembered if this very version was hashed before, otherwise
    /// computed, and checked not to have changed while it was read.
    pub async fn hash_opened(&self, opened: &Opened) -> Result<Hash> {
        let version = opened.meta.identity();
        if let Some(hash) = self.inner.cache.get(&opened.real, &version) {
            return Ok(hash);
        }
        let file = Arc::clone(&opened.file);
        let length = opened.meta.size;
        let hash = blocking(move || {
            let hash = hash_file_prefix(&file, length)
                .map_err(|error| HostError::io("reading a file to hash it", error))?;
            let after = file_meta(&file)
                .map_err(|error| HostError::io("looking at a file after hashing it", error))?;
            Ok((hash, after.identity()))
        })
        .await?;
        if hash.1 != version {
            return Err(HostError::Changed(format!(
                "{} changed while it was being read",
                opened.real
            )));
        }
        self.inner.cache.put(&opened.real, version, hash.0);
        Ok(hash.0)
    }

    /// Hashes the file at `path` and puts its content into the store unless the store holds it
    /// already. With `expect`, a file that is another version than that one is not sent.
    /// `max_bytes` can only narrow the host's own file limit.
    pub async fn fetch(
        &self,
        path: &RelPath,
        expect: Option<FileVersion>,
        max_bytes: Option<i64>,
        sink: &dyn BlobSink,
    ) -> Result<FsFetchResult> {
        if max_bytes.is_some_and(|limit| limit < 0) {
            return Err(HostError::Unsupported(
                "the file size limit must be zero or greater".to_owned(),
            ));
        }
        let limit = max_bytes
            .unwrap_or(self.inner.limits.max_blob_bytes)
            .min(self.inner.limits.max_blob_bytes);
        let this = self.clone();
        let asked = path.clone();
        let opened = blocking(move || this.open_regular(&asked)).await?;
        let size = i64::try_from(opened.meta.size).unwrap_or(i64::MAX);
        if let Some(expected) = expect
            && (expected.size != size || expected.mtime_ns != opened.meta.mtime_ns)
        {
            return Err(HostError::Changed(format!(
                "{path} is not the version that was asked for"
            )));
        }
        if size > limit {
            return Err(HostError::TooLarge {
                what: path.to_string(),
                size,
                limit,
            });
        }
        let hash = self.hash_opened(&opened).await?;
        let mut reader = FileReader::new(Arc::clone(&opened.file));
        let sent = self
            .inner
            .uploads
            .ensure(sink, hash, size, &mut reader)
            .await;
        let file = Arc::clone(&opened.file);
        let after = blocking(move || {
            file_meta(&file)
                .map_err(|error| HostError::io("looking at a file after sending it", error))
        })
        .await?;
        if after.identity() != opened.meta.identity() {
            return Err(HostError::Changed(format!(
                "{path} changed while it was being sent; try reading it again"
            )));
        }
        sent.map_err(blob_failure)?;
        Ok(FsFetchResult {
            hash,
            size,
            mtime_ns: opened.meta.mtime_ns,
        })
    }

    // ------------------------------------------------------------------ writing

    /// What is at the target of a write: its hash if it is a regular file, `None` if nothing is.
    fn current_hash(&self, parent: &Dir, name: &OsStr, real: &RelPath) -> Result<Option<Hash>> {
        let meta = match parent.lstat(name) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(HostError::io(format!("looking at {real}"), error)),
        };
        if !meta.is_file() {
            return Err(HostError::Exists(format!(
                "{real} is there and is not a file"
            )));
        }
        let file = parent
            .open_read(name)
            .map_err(|error| HostError::io(format!("opening {real}"), error))?;
        let version = file_meta(&file)
            .map_err(|error| HostError::io(format!("looking at {real}"), error))?
            .identity();
        if let Some(hash) = self.inner.cache.get(real, &version) {
            return Ok(Some(hash));
        }
        let hash =
            hash_file(&file).map_err(|error| HostError::io(format!("reading {real}"), error))?;
        self.inner.cache.put(real, version, hash);
        Ok(Some(hash))
    }

    /// Whether the file at the target is the version the writer saw.
    fn matches_expected(
        &self,
        parent: &Dir,
        name: &OsStr,
        real: &RelPath,
        expected: Option<ExpectedHash>,
    ) -> Result<bool> {
        let Some(expected) = expected else {
            return Ok(true);
        };
        let current = self.current_hash(parent, name, real)?;
        Ok(match expected {
            ExpectedHash::Absent => current.is_none(),
            ExpectedHash::Is(hash) => current == Some(hash),
        })
    }

    fn stale(&self, path: &RelPath) -> HostError {
        let current = self.stat(path).ok().map(|mut stat| {
            if stat.hash.is_none() {
                stat.hash = self
                    .ws()
                    .locate(path, Locate::THROUGH)
                    .ok()
                    .and_then(|located| {
                        self.current_hash(&located.parent, located.name().ok()?, &located.real)
                            .ok()
                            .flatten()
                    });
            }
            Box::new(stat)
        });
        HostError::Stale { current }
    }

    /// Writes the content `blob` holds to the file at `path`, creating the directories above it.
    /// With `expected` set, only when the file there is that version.
    pub async fn write(
        &self,
        path: &RelPath,
        blob: Hash,
        mode: Option<u32>,
        expected: Option<ExpectedHash>,
        source: &dyn BlobSource,
    ) -> Result<FileStat> {
        self.require_write()?;
        let this = self.clone();
        let asked = path.clone();
        let prepared = blocking(move || {
            let located = this.ws().locate(&asked, Locate::FOR_WRITE)?;
            if located.name.is_none() {
                return Err(HostError::Unsupported(
                    "the workspace root is not a file".to_owned(),
                ));
            }
            this.check_visible(&asked, &located, false)?;
            if located.meta.is_some_and(|meta| meta.is_dir()) {
                return Err(HostError::Exists(format!("{asked} is a directory")));
            }
            let name = located.name()?.to_owned();
            if !this.matches_expected(&located.parent, &name, &located.real, expected)? {
                return Err(this.stale(&asked));
            }
            let existing_mode = located.meta.map(|meta| meta.mode);
            Ok((located.parent, name, located.real, existing_mode))
        })
        .await?;
        let (parent, name, real, existing_mode) = prepared;

        let temp = temp_name();
        let file = parent
            .create_new(&temp, NEW_FILE_MODE)
            .map_err(|error| HostError::io(format!("receiving {path}"), error))?;
        let mut cleanup = Cleanup::new(&parent, &temp, false)?;
        let file = Arc::new(file);
        let mut writer = FileWriter::new(Arc::clone(&file));
        download_checked(source, blob, &mut writer)
            .await
            .map_err(blob_failure)?;

        let this = self.clone();
        let asked = path.clone();
        let finished = Arc::clone(&file);
        let stat = blocking(move || {
            let bits = mode
                .map(|bits| bits & WRITABLE_MODE_BITS)
                .or_else(|| existing_mode.map(|bits| bits & WRITABLE_MODE_BITS));
            if let Some(bits) = bits {
                set_file_mode(&finished, bits).map_err(|error| {
                    HostError::io("setting the permissions of a new file", error)
                })?;
            }
            finished
                .sync_all()
                .map_err(|error| HostError::io(format!("flushing {asked}"), error))?;
            // The version is checked once more as late as it can be: between here and the rename is
            // the only moment another writer could still get in.
            if !this.matches_expected(&parent, &name, &real, expected)? {
                return Err(this.stale(&asked));
            }
            let replace = !matches!(expected, Some(ExpectedHash::Absent));
            parent
                .rename_to(&temp, &parent, &name, replace)
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::AlreadyExists {
                        this.stale(&asked)
                    } else {
                        HostError::io(format!("putting {asked} in place"), error)
                    }
                })?;
            cleanup.disarm();
            let _ = parent.sync();
            let after = file_meta(&finished)
                .map_err(|error| HostError::io(format!("looking at {asked}"), error))?;
            this.inner.cache.put(&real, after.identity(), blob);
            this.note_orch_write(&real);
            let mut stat = this.stat(&asked)?;
            stat.hash = Some(blob);
            Ok(stat)
        })
        .await?;
        Ok(stat)
    }

    /// Makes the directory at `path`, and the directories above it that are missing. A directory
    /// that is there already is not an error.
    pub fn make_dir(&self, path: &RelPath) -> Result<FileStat> {
        self.require_write()?;
        let located = self.ws().locate(path, Locate::FOR_CREATE)?;
        if located.name.is_none() {
            return self.stat(path);
        }
        self.check_visible(path, &located, true)?;
        match located.meta {
            Some(meta) if meta.is_dir() => {}
            Some(_) => {
                return Err(HostError::Exists(format!(
                    "{path} is there and is not a directory"
                )));
            }
            None => located
                .parent
                .mkdir(located.name()?, NEW_DIR_MODE)
                .map_err(|error| HostError::io(format!("making {path}"), error))?,
        }
        self.note_orch_write(&located.real);
        self.stat(path)
    }

    /// Renames the entry at `from` to `to`, which must not exist. A link is renamed, not what it
    /// leads to.
    pub fn rename(&self, from: &RelPath, to: &RelPath) -> Result<FileStat> {
        self.require_write()?;
        let source = self.ws().locate(from, Locate::ENTRY)?;
        let source_meta = source
            .meta
            .ok_or_else(|| HostError::NotFound(format!("{from} is not there")))?;
        let source_name = source.name()?.to_owned();
        self.check_visible(from, &source, source_meta.is_dir())?;
        let target = self.ws().locate(to, Locate::ENTRY)?;
        let target_name = target
            .name()
            .map_err(|_| HostError::Exists("the workspace root is there".to_owned()))?
            .to_owned();
        self.check_visible(to, &target, source_meta.is_dir())?;
        if source_meta.is_dir() && target.real.starts_with(&source.real) {
            return Err(HostError::Exists(format!(
                "{from} cannot be moved into itself"
            )));
        }
        let mut replace = false;
        if let Some(there) = target.meta {
            // On a file system that does not tell case apart, a name changed only in case is the
            // same file under a new spelling, and that is allowed.
            // Where the system numbers no files, the same directory and the same name but for case
            // is the same file.
            let same_file = if there.ino != 0 {
                there.dev == source_meta.dev && there.ino == source_meta.ino
            } else {
                source.real.parent() == target.real.parent()
                    && source_name.to_string_lossy().to_lowercase()
                        == target_name.to_string_lossy().to_lowercase()
            };
            if !same_file || self.ws().case_sensitive() {
                return Err(HostError::Exists(format!("{to} is there already")));
            }
            replace = true;
        }
        match source
            .parent
            .rename_to(&source_name, &target.parent, &target_name, replace)
        {
            Ok(()) => {}
            Err(error) if is_cross_device(&error) => {
                self.copy(from, to)?;
                self.remove_checked(from, true)?;
            }
            Err(error) => return Err(HostError::io(format!("renaming {from}"), error)),
        }
        self.inner.cache.forget(&source.real);
        self.note_orch_write(&target.real);
        self.stat(to)
    }

    /// Removes the entry at `path`; a directory with what is in it only if `recursive` is set. A
    /// link is removed, not what it leads to.
    pub fn remove(&self, path: &RelPath, recursive: bool) -> Result<()> {
        self.require_write()?;
        self.remove_checked(path, recursive)
    }

    fn remove_checked(&self, path: &RelPath, recursive: bool) -> Result<()> {
        let located = self.ws().locate(path, Locate::ENTRY)?;
        let meta = located
            .meta
            .ok_or_else(|| HostError::NotFound(format!("{path} is not there")))?;
        let name = located.name()?;
        self.check_visible(path, &located, meta.is_dir())?;
        if meta.is_dir() {
            if recursive {
                remove_tree(&located.parent, name, 0)?;
            } else {
                located
                    .parent
                    .rmdir(name)
                    .map_err(|error| HostError::io(format!("removing {path}"), error))?;
            }
        } else {
            located
                .parent
                .unlink(name)
                .map_err(|error| HostError::io(format!("removing {path}"), error))?;
        }
        self.inner.cache.forget(&located.real);
        Ok(())
    }

    /// Copies the entry at `from` to `to`, which must not exist: a file, or a directory with what
    /// is in it that the ignore rules do not leave out. The copy appears whole or not at all.
    pub fn copy(&self, from: &RelPath, to: &RelPath) -> Result<FileStat> {
        self.require_write()?;
        let source = self.ws().locate(from, Locate::ENTRY)?;
        let source_meta = source
            .meta
            .ok_or_else(|| HostError::NotFound(format!("{from} is not there")))?;
        let source_name = source.name()?.to_owned();
        self.check_visible(from, &source, source_meta.is_dir())?;
        let target = self.ws().locate(to, Locate::ENTRY)?;
        let target_name = target
            .name()
            .map_err(|_| HostError::Exists("the workspace root is there".to_owned()))?
            .to_owned();
        self.check_visible(to, &target, source_meta.is_dir())?;
        if target.meta.is_some() {
            return Err(HostError::Exists(format!("{to} is there already")));
        }
        if source_meta.is_dir() && target.real.starts_with(&source.real) {
            return Err(HostError::Exists(format!(
                "{from} cannot be copied into itself"
            )));
        }
        let scope = match (&self.inner.ignore, source_meta.is_dir()) {
            (Some(rules), true) => rules.scope_at(self.ws(), &source.real),
            _ => None,
        };
        let temp = temp_name();
        let mut cleanup = Cleanup::new(&target.parent, &temp, source_meta.is_dir())?;
        let mut budget = Budget {
            left: self.inner.limits.max_blob_bytes,
            limit: self.inner.limits.max_blob_bytes,
            what: from.to_string(),
        };
        self.copy_entry(
            &source.parent,
            &source_name,
            &source_meta,
            &source.real,
            scope.as_ref(),
            &target.parent,
            &temp,
            0,
            &mut budget,
        )?;
        target
            .parent
            .rename_to(&temp, &target.parent, &target_name, false)
            .map_err(|error| HostError::io(format!("putting the copy {to} in place"), error))?;
        cleanup.disarm();
        let _ = target.parent.sync();
        self.note_orch_write(&target.real);
        self.stat(to)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "a recursive copy carries its whole position with it"
    )]
    fn copy_entry(
        &self,
        from_dir: &Dir,
        from_name: &OsStr,
        meta: &Meta,
        real: &RelPath,
        scope: Option<&crate::ignore::Scope>,
        to_dir: &Dir,
        to_name: &OsStr,
        depth: usize,
        budget: &mut Budget,
    ) -> Result<()> {
        match meta.kind {
            Kind::File => {
                let source = from_dir
                    .open_read(from_name)
                    .map_err(|error| HostError::io(format!("opening {real}"), error))?;
                budget.check(meta.size)?;
                let target = to_dir
                    .create_new(to_name, meta.mode & WRITABLE_MODE_BITS)
                    .map_err(|error| HostError::io("making the copy of a file", error))?;
                let mut offset = 0u64;
                let mut buffer = vec![0u8; COPY_CHUNK_BYTES];
                loop {
                    crate::error::check_cancelled()?;
                    let count = read_at(&source, &mut buffer, offset)
                        .map_err(|error| HostError::io(format!("reading {real}"), error))?;
                    if count == 0 {
                        break;
                    }
                    // A file can grow after its metadata was read; only bytes actually read
                    // count toward the shared budget, before any of this chunk is written.
                    budget.take(count as u64)?;
                    let mut written = 0usize;
                    while written < count {
                        let step =
                            write_at(&target, &buffer[written..count], offset + written as u64)
                                .map_err(|error| {
                                    HostError::io("writing the copy of a file", error)
                                })?;
                        if step == 0 {
                            return Err(HostError::io(
                                "writing the copy of a file",
                                std::io::Error::from(std::io::ErrorKind::WriteZero),
                            ));
                        }
                        written += step;
                    }
                    offset += count as u64;
                }
                let _ = target.set_modified(
                    std::time::UNIX_EPOCH
                        + std::time::Duration::from_nanos(
                            u64::try_from(meta.mtime_ns).unwrap_or(0),
                        ),
                );
                Ok(())
            }
            Kind::Symlink => {
                let target = from_dir
                    .read_link(from_name)
                    .map_err(|error| HostError::io(format!("reading the link {real}"), error))?;
                to_dir
                    .symlink(&target, to_name)
                    .map_err(|error| HostError::io("making the copy of a link", error))
            }
            Kind::Dir => {
                if depth >= WALK_DEPTH_MAX {
                    return Err(HostError::Unsupported(
                        "a directory tree this deep is not copied".to_owned(),
                    ));
                }
                to_dir
                    .mkdir(to_name, COPYING_DIR_MODE)
                    .map_err(|error| HostError::io("making the copy of a directory", error))?;
                let from_child = from_dir
                    .open_child(from_name)
                    .map_err(|error| HostError::io(format!("entering {real}"), error))?;
                let to_child = to_dir
                    .open_child(to_name)
                    .map_err(|error| HostError::io("entering the copy of a directory", error))?;
                let mut names = from_child
                    .names()
                    .map_err(|error| HostError::io(format!("listing {real}"), error))?;
                names.sort();
                for name in names {
                    let Some(text) = name.to_str() else { continue };
                    if is_internal_name(text) {
                        continue;
                    }
                    let Ok(child_real) = real.join(text) else {
                        continue;
                    };
                    let Ok(child_meta) = from_child.lstat(&name) else {
                        continue;
                    };
                    if let Some(scope) = scope
                        && scope.is_ignored(&child_real, child_meta.is_dir())
                    {
                        continue;
                    }
                    let child_scope = match (scope, child_meta.is_dir()) {
                        (Some(scope), true) => Some(scope.enter(
                            &child_real,
                            &from_child.open_child(&name).map_err(|error| {
                                HostError::io(format!("entering {child_real}"), error)
                            })?,
                        )),
                        _ => None,
                    };
                    self.copy_entry(
                        &from_child,
                        &name,
                        &child_meta,
                        &child_real,
                        child_scope.as_ref(),
                        &to_child,
                        &name,
                        depth + 1,
                        budget,
                    )?;
                }
                let _ = to_dir.set_mode(to_name, meta.mode & WRITABLE_MODE_BITS);
                let _ = to_dir.set_mtime(to_name, meta.mtime_ns);
                Ok(())
            }
            Kind::Other => Ok(()),
        }
    }
}

/// How many bytes a copy may still move before it passes what Orch stores.
struct Budget {
    left: i64,
    limit: i64,
    what: String,
}

impl Budget {
    fn check(&self, bytes: u64) -> Result<()> {
        let bytes = i64::try_from(bytes).unwrap_or(i64::MAX);
        if bytes > self.left {
            return Err(HostError::TooLarge {
                what: self.what.clone(),
                size: self.limit.saturating_sub(self.left).saturating_add(bytes),
                limit: self.limit,
            });
        }
        Ok(())
    }

    fn take(&mut self, bytes: u64) -> Result<()> {
        self.check(bytes)?;
        self.left -= i64::try_from(bytes).unwrap_or(i64::MAX);
        Ok(())
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::blobs::MemoryBlobs;
    use crate::environment::Gates;
    use crate::paths::PathStyle;
    use std::path::Path;

    struct Fixture {
        _root: tempfile::TempDir,
        dir: std::path::PathBuf,
        files: Files,
        blobs: MemoryBlobs,
    }

    fn fixture_with(patterns: &[&str], gates: Gates) -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().canonicalize().unwrap();
        let ws = Workspace::open(&dir, &[], PathStyle::Posix, true).unwrap();
        let rules = Arc::new(IgnoreRules::new(
            &patterns.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>(),
            true,
        ));
        let files = Files::workspace(
            ws,
            rules,
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(gates)),
        );
        Fixture {
            _root: root,
            dir,
            files,
            blobs: MemoryBlobs::new(),
        }
    }

    fn fixture() -> Fixture {
        fixture_with(&[], Gates::OPEN)
    }

    fn path(text: &str) -> RelPath {
        RelPath::parse(text, PathStyle::Posix).unwrap()
    }

    fn read(dir: &Path, name: &str) -> String {
        std::fs::read_to_string(dir.join(name)).unwrap()
    }

    #[test]
    fn a_listing_has_directories_first_then_names_without_regard_to_case_and_marks_links() {
        let f = fixture();
        std::fs::create_dir_all(f.dir.join("Zeta")).unwrap();
        std::fs::create_dir_all(f.dir.join("alpha")).unwrap();
        std::fs::write(f.dir.join("b.txt"), "bb").unwrap();
        std::fs::write(f.dir.join("A.txt"), "a").unwrap();
        std::os::unix::fs::symlink("b.txt", f.dir.join("link")).unwrap();
        let listing = f.files.list(&RelPath::root()).unwrap();
        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["alpha", "Zeta", "A.txt", "b.txt", "link"]);
        assert!(!listing.truncated);
        let link = listing.entries.iter().find(|e| e.name == "link").unwrap();
        assert_eq!(link.kind, DirEntryKind::Symlink);
        assert_eq!(link.link_target.as_deref(), Some("b.txt"));
        assert_eq!(
            link.size, 2,
            "a link to a file shows the version of the file, which is what a read of it will be"
        );
    }

    #[test]
    fn a_listing_stops_at_five_thousand_entries_and_says_so() {
        let f = fixture();
        for n in 0..5001 {
            std::fs::write(f.dir.join(format!("f{n:05}")), "").unwrap();
        }
        let listing = f.files.list(&RelPath::root()).unwrap();
        assert_eq!(listing.entries.len(), 5000);
        assert!(listing.truncated);
    }

    #[test]
    fn an_ignored_file_is_neither_listed_nor_readable_nor_writable_and_a_link_does_not_lead_to_it()
    {
        let f = fixture_with(&["secret/", "*.key"], Gates::OPEN);
        std::fs::create_dir_all(f.dir.join("secret")).unwrap();
        std::fs::write(f.dir.join("secret/token"), "t").unwrap();
        std::fs::write(f.dir.join("id.key"), "k").unwrap();
        std::fs::write(f.dir.join("shown.txt"), "s").unwrap();
        std::os::unix::fs::symlink("secret/token", f.dir.join("innocent")).unwrap();
        let names: Vec<String> = f
            .files
            .list(&RelPath::root())
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["innocent", "shown.txt"]);
        for hidden in ["secret", "secret/token", "id.key"] {
            assert!(
                matches!(f.files.stat(&path(hidden)), Err(HostError::Ignored(_))),
                "{hidden} must not be described"
            );
        }
        assert!(matches!(
            f.files.list(&path("secret")),
            Err(HostError::Ignored(_))
        ));
        assert!(matches!(
            f.files.open_regular(&path("id.key")),
            Err(HostError::Ignored(_))
        ));
        assert!(
            matches!(
                f.files.open_regular(&path("innocent")),
                Err(HostError::Ignored(_))
            ),
            "a link cannot be used to read what is left out"
        );
        let blob = f.blobs.insert(&b"new"[..]);
        let written = block_on(f.files.write(&path("id.key"), blob, None, None, &f.blobs));
        assert!(matches!(written, Err(HostError::Ignored(_))));
        assert!(matches!(
            f.files.make_dir(&path("secret/new")),
            Err(HostError::Ignored(_))
        ));
        assert!(matches!(
            f.files.rename(&path("shown.txt"), &path("moved.key")),
            Err(HostError::Ignored(_))
        ));
        assert_eq!(read(&f.dir, "id.key"), "k");
    }

    /// Runs one future to its end on a runtime of its own, for the tests of the parts that are async.
    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn a_stat_tells_the_media_type_and_whether_the_file_is_text_and_a_hash_only_when_one_is_known()
    {
        let f = fixture();
        std::fs::write(f.dir.join("notes.md"), "# hello\n").unwrap();
        std::fs::write(f.dir.join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
        std::fs::create_dir(f.dir.join("sub")).unwrap();
        let notes = f.files.stat(&path("notes.md")).unwrap();
        assert_eq!(notes.media_type, "text/markdown");
        assert!(notes.is_text);
        assert_eq!(notes.hash, None, "nothing hashed it yet");
        assert_eq!(notes.entry.size, 8);
        let blob = f.files.stat(&path("blob.bin")).unwrap();
        assert!(!blob.is_text);
        assert_eq!(
            f.files.stat(&path("sub")).unwrap().media_type,
            "inode/directory"
        );
        assert!(matches!(
            f.files.stat(&path("missing")),
            Err(HostError::NotFound(_))
        ));
    }

    #[test]
    fn fetching_a_file_uploads_only_what_the_store_lacks_and_refuses_another_version() {
        let f = fixture();
        std::fs::write(f.dir.join("a.txt"), "first content").unwrap();
        let result = block_on(f.files.fetch(&path("a.txt"), None, None, &f.blobs)).unwrap();
        assert_eq!(result.hash, Hash::of(b"first content"));
        assert_eq!(f.blobs.uploads(), vec![result.hash]);
        // The same content again is not uploaded again, and the hash is now known to a stat.
        block_on(f.files.fetch(&path("a.txt"), None, None, &f.blobs)).unwrap();
        assert_eq!(f.blobs.uploads().len(), 1);
        assert_eq!(
            f.files.stat(&path("a.txt")).unwrap().hash,
            Some(result.hash)
        );
        // A file that is another version than the one asked for is not sent.
        let wrong = FileVersion {
            size: result.size + 1,
            mtime_ns: result.mtime_ns,
        };
        assert!(matches!(
            block_on(f.files.fetch(&path("a.txt"), Some(wrong), None, &f.blobs)),
            Err(HostError::Changed(_))
        ));
        let right = FileVersion {
            size: result.size,
            mtime_ns: result.mtime_ns,
        };
        assert!(block_on(f.files.fetch(&path("a.txt"), Some(right), None, &f.blobs)).is_ok());
    }

    #[test]
    fn a_file_over_the_size_limit_is_listed_and_not_read() {
        let mut f = fixture();
        let limits = Limits {
            max_blob_bytes: 4,
            ..Limits::default()
        };
        let ws = Workspace::open(&f.dir, &[], PathStyle::Posix, true).unwrap();
        f.files = Files::workspace(
            ws,
            Arc::new(IgnoreRules::new(&[], true)),
            Arc::new(HashCache::new()),
            limits,
            Arc::new(GateState::new(Gates::OPEN)),
        );
        std::fs::write(f.dir.join("big"), "123456789").unwrap();
        assert_eq!(f.files.list(&RelPath::root()).unwrap().entries.len(), 1);
        assert!(matches!(
            block_on(f.files.fetch(&path("big"), None, None, &f.blobs)),
            Err(HostError::TooLarge { .. })
        ));
        assert!(
            matches!(
                block_on(f.files.fetch(&path("big"), None, Some(100), &f.blobs)),
                Err(HostError::TooLarge {
                    size: 9,
                    limit: 4,
                    ..
                })
            ),
            "a caller cannot raise the native file limit"
        );
        assert!(f.blobs.uploads().is_empty());
    }

    struct GrowingSink<'a> {
        file: std::path::PathBuf,
        blobs: &'a MemoryBlobs,
    }

    #[async_trait::async_trait]
    impl BlobSink for GrowingSink<'_> {
        async fn missing(&self, hashes: &[Hash]) -> std::result::Result<Vec<Hash>, BlobError> {
            self.blobs.missing(hashes).await
        }

        async fn put(
            &self,
            hash: Hash,
            size: i64,
            source: &mut dyn crate::blobs::BlobReader,
        ) -> std::result::Result<(), BlobError> {
            use std::io::Write;
            assert_eq!(size, 4, "the upload stays bounded to the captured size");
            std::fs::OpenOptions::new()
                .append(true)
                .open(&self.file)
                .and_then(|mut file| file.write_all(b"grew"))
                .map_err(|error| BlobError::Local(error.to_string()))?;
            self.blobs.put(hash, size, source).await
        }
    }

    #[test]
    fn a_file_that_grows_during_upload_is_not_returned_as_a_successful_attachment()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let f = fixture();
        let file = f.dir.join("result.bin");
        std::fs::write(&file, b"1234")?;
        let sink = GrowingSink {
            file: file.clone(),
            blobs: &f.blobs,
        };
        let result = block_on(f.files.fetch(&path("result.bin"), None, Some(4), &sink));
        let Err(HostError::Changed(detail)) = result else {
            panic!("the changed file must not be acknowledged: {result:?}");
        };
        assert!(detail.contains("result.bin") && detail.contains("try reading it again"));
        assert_eq!(std::fs::read(file)?, b"1234grew");
        let uploaded = f
            .blobs
            .get_bytes(&Hash::of(b"1234"))
            .ok_or("the captured content is missing")?;
        assert_eq!(uploaded.as_ref(), b"1234");
        assert!(!f.blobs.holds(&Hash::of(b"1234grew")));
        Ok(())
    }

    #[test]
    fn a_write_replaces_the_file_whole_and_a_write_on_a_stale_version_is_refused_and_changes_nothing()
     {
        let f = fixture();
        std::fs::write(f.dir.join("doc.txt"), "version one").unwrap();
        let one = Hash::of(b"version one");
        let two_blob = f.blobs.insert(&b"version two"[..]);
        let three_blob = f.blobs.insert(&b"version three"[..]);

        // Written on the version the editor loaded: accepted, and the answer knows the hash.
        let stat = block_on(f.files.write(
            &path("doc.txt"),
            two_blob,
            None,
            Some(ExpectedHash::Is(one)),
            &f.blobs,
        ))
        .unwrap();
        assert_eq!(read(&f.dir, "doc.txt"), "version two");
        assert_eq!(stat.hash, Some(two_blob));

        // The same writer, still on version one, is refused; the file stays as the other left it.
        let refused = block_on(f.files.write(
            &path("doc.txt"),
            three_blob,
            None,
            Some(ExpectedHash::Is(one)),
            &f.blobs,
        ));
        match refused {
            Err(HostError::Stale {
                current: Some(current),
            }) => assert_eq!(current.entry.size, 11, "with the file as it stands"),
            other => panic!("expected a stale refusal, got {other:?}"),
        }
        assert_eq!(read(&f.dir, "doc.txt"), "version two");
        let leftovers: Vec<_> = std::fs::read_dir(&f.dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            leftovers.len(),
            1,
            "no half-written file is left beside it: {leftovers:?}"
        );

        // "Must not exist" holds only for a file that is not there.
        let absent = block_on(f.files.write(
            &path("doc.txt"),
            three_blob,
            None,
            Some(ExpectedHash::Absent),
            &f.blobs,
        ));
        assert!(matches!(absent, Err(HostError::Stale { .. })));
        block_on(f.files.write(
            &path("new/dir/file.txt"),
            three_blob,
            None,
            Some(ExpectedHash::Absent),
            &f.blobs,
        ))
        .unwrap();
        assert_eq!(read(&f.dir, "new/dir/file.txt"), "version three");
    }

    #[test]
    fn a_write_that_is_cut_off_leaves_the_old_file_whole_and_removes_what_it_had_received() {
        use crate::blobs::BlobWriter;
        use async_trait::async_trait;

        /// A store whose download sends half of the content and then fails, as a link that is cut does.
        struct Cut;
        #[async_trait]
        impl BlobSource for Cut {
            async fn get(
                &self,
                _hash: Hash,
                target: &mut dyn BlobWriter,
            ) -> std::result::Result<i64, BlobError> {
                target
                    .write(0, bytes::Bytes::from_static(b"half of"))
                    .await
                    .map_err(|e| BlobError::Local(e.to_string()))?;
                Err(BlobError::StoodStill)
            }
        }
        let f = fixture();
        std::fs::write(f.dir.join("keep.txt"), "the old content").unwrap();
        let result = block_on(f.files.write(
            &path("keep.txt"),
            Hash::of(b"half of the new"),
            None,
            None,
            &Cut,
        ));
        assert!(matches!(result, Err(HostError::Content(_))));
        assert_eq!(read(&f.dir, "keep.txt"), "the old content");
        let names: Vec<_> = std::fs::read_dir(&f.dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            names,
            [std::ffi::OsString::from("keep.txt")],
            "the received half is removed"
        );

        // Content that arrives whole but is not what was named is not written either.
        let wrong = f.blobs.insert(&b"something else"[..]);
        let fake = Hash::of(b"what was promised");
        let lie = block_on(async {
            struct Swap(MemoryBlobs, Hash);
            #[async_trait]
            impl BlobSource for Swap {
                async fn get(
                    &self,
                    _hash: Hash,
                    target: &mut dyn BlobWriter,
                ) -> std::result::Result<i64, BlobError> {
                    self.0.get(self.1, target).await
                }
            }
            f.files
                .write(
                    &path("keep.txt"),
                    fake,
                    None,
                    None,
                    &Swap(f.blobs.clone(), wrong),
                )
                .await
        });
        assert!(matches!(lie, Err(HostError::Changed(_))));
        assert_eq!(read(&f.dir, "keep.txt"), "the old content");
    }

    #[test]
    fn a_write_keeps_the_permissions_of_the_file_it_replaces_and_never_sets_the_special_bits() {
        use std::os::unix::fs::PermissionsExt;
        let f = fixture();
        std::fs::write(f.dir.join("run.sh"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(f.dir.join("run.sh"), std::fs::Permissions::from_mode(0o750))
            .unwrap();
        let blob = f.blobs.insert(&b"#!/bin/sh\necho hi\n"[..]);
        block_on(f.files.write(&path("run.sh"), blob, None, None, &f.blobs)).unwrap();
        assert_eq!(
            std::fs::metadata(f.dir.join("run.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o750
        );
        block_on(
            f.files
                .write(&path("run.sh"), blob, Some(0o4755), None, &f.blobs),
        )
        .unwrap();
        assert_eq!(
            std::fs::metadata(f.dir.join("run.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o755,
            "no set-user-id from outside"
        );
    }

    #[test]
    fn nothing_is_written_renamed_copied_or_removed_where_the_machine_accepts_no_writes_from_outside()
     {
        let f = fixture_with(&[], Gates::CLOSED);
        std::fs::write(f.dir.join("a"), "a").unwrap();
        let blob = f.blobs.insert(&b"x"[..]);
        assert!(matches!(
            block_on(f.files.write(&path("a"), blob, None, None, &f.blobs)),
            Err(HostError::RemoteWriteDisabled)
        ));
        assert!(matches!(
            f.files.make_dir(&path("d")),
            Err(HostError::RemoteWriteDisabled)
        ));
        assert!(matches!(
            f.files.rename(&path("a"), &path("b")),
            Err(HostError::RemoteWriteDisabled)
        ));
        assert!(matches!(
            f.files.copy(&path("a"), &path("b")),
            Err(HostError::RemoteWriteDisabled)
        ));
        assert!(matches!(
            f.files.remove(&path("a"), false),
            Err(HostError::RemoteWriteDisabled)
        ));
        assert_eq!(read(&f.dir, "a"), "a");
        // Reading is not writing.
        assert!(f.files.stat(&path("a")).is_ok());
    }

    #[test]
    fn a_rename_or_a_copy_does_not_make_the_directory_it_moves_into() {
        let f = fixture();
        std::fs::write(f.dir.join("a.txt"), "a").unwrap();
        assert!(matches!(
            f.files.rename(&path("a.txt"), &path("sub/c.txt")),
            Err(HostError::NotFound(_))
        ));
        assert!(matches!(
            f.files.copy(&path("a.txt"), &path("sub/c.txt")),
            Err(HostError::NotFound(_))
        ));
        assert!(!f.dir.join("sub").exists());
        assert_eq!(read(&f.dir, "a.txt"), "a");
    }

    #[test]
    fn rename_and_copy_refuse_a_destination_that_exists_and_a_directory_into_itself() {
        let f = fixture();
        std::fs::create_dir_all(f.dir.join("src/inner")).unwrap();
        std::fs::write(f.dir.join("src/inner/f.txt"), "f").unwrap();
        std::fs::write(f.dir.join("a.txt"), "a").unwrap();
        std::fs::write(f.dir.join("b.txt"), "b").unwrap();
        assert!(matches!(
            f.files.rename(&path("a.txt"), &path("b.txt")),
            Err(HostError::Exists(_))
        ));
        assert!(matches!(
            f.files.copy(&path("a.txt"), &path("b.txt")),
            Err(HostError::Exists(_))
        ));
        assert!(matches!(
            f.files.rename(&path("src"), &path("src/inner/src")),
            Err(HostError::Exists(_))
        ));
        assert!(matches!(
            f.files.copy(&path("src"), &path("src/inner/again")),
            Err(HostError::Exists(_))
        ));
        assert_eq!(
            (read(&f.dir, "a.txt"), read(&f.dir, "b.txt")),
            ("a".to_owned(), "b".to_owned())
        );
        assert!(matches!(
            f.files.rename(&RelPath::root(), &path("x")),
            Err(HostError::Unsupported(_))
        ));
    }

    #[test]
    fn a_file_is_renamed_moved_and_copied_and_a_directory_goes_with_what_is_in_it() {
        let f = fixture();
        std::fs::create_dir_all(f.dir.join("src/inner")).unwrap();
        std::fs::write(f.dir.join("src/inner/f.txt"), "deep").unwrap();
        std::fs::write(f.dir.join("src/top.txt"), "top").unwrap();
        std::os::unix::fs::symlink("top.txt", f.dir.join("src/ln")).unwrap();
        std::fs::create_dir(f.dir.join("dest")).unwrap();

        f.files
            .rename(&path("src/top.txt"), &path("dest/moved.txt"))
            .unwrap();
        assert!(!f.dir.join("src/top.txt").exists());
        assert_eq!(read(&f.dir, "dest/moved.txt"), "top");

        let copy = f.files.copy(&path("src"), &path("copy")).unwrap();
        assert_eq!(copy.entry.kind, DirEntryKind::Dir);
        assert_eq!(read(&f.dir, "copy/inner/f.txt"), "deep");
        assert_eq!(
            read(&f.dir, "src/inner/f.txt"),
            "deep",
            "the original is as it was"
        );
        assert_eq!(
            std::fs::read_link(f.dir.join("copy/ln")).unwrap(),
            Path::new("top.txt"),
            "a link is copied as a link"
        );

        f.files.rename(&path("copy"), &path("renamed")).unwrap();
        assert!(f.dir.join("renamed/inner/f.txt").exists() && !f.dir.join("copy").exists());
        let names: Vec<_> = std::fs::read_dir(&f.dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert!(
            names.iter().all(|n| !n.starts_with(TEMP_PREFIX)),
            "no half-made copy is left: {names:?}"
        );
    }

    #[test]
    fn a_copy_refuses_a_file_that_grows_after_its_size_was_checked() {
        let f = fixture();
        std::fs::write(f.dir.join("source"), "a").unwrap();
        let source = f.files.ws().locate(&path("source"), Locate::ENTRY).unwrap();
        let meta = source.meta.unwrap();
        std::fs::write(f.dir.join("source"), "larger").unwrap();

        let temporary = OsStr::new(".copy-in-progress");
        let destination = OsStr::new("copy");
        let mut cleanup = Cleanup::new(&source.parent, temporary, false).unwrap();
        let mut budget = Budget {
            left: 4,
            limit: 4,
            what: "source".to_owned(),
        };
        let result = f.files.copy_entry(
            &source.parent,
            source.name().unwrap(),
            &meta,
            &source.real,
            None,
            &source.parent,
            temporary,
            0,
            &mut budget,
        );
        if result.is_ok() {
            source
                .parent
                .rename_to(temporary, &source.parent, destination, false)
                .unwrap();
            cleanup.disarm();
        }
        drop(cleanup);

        assert!(matches!(result, Err(HostError::TooLarge { limit: 4, .. })));
        assert!(!f.dir.join("copy").exists());
        assert!(!f.dir.join(temporary).exists());
        assert_eq!(read(&f.dir, "source"), "larger");
    }

    #[test]
    fn a_copy_counts_actual_bytes_across_a_tree_and_accepts_the_limit() {
        let mut f = fixture();
        let ws = Workspace::open(&f.dir, &[], PathStyle::Posix, true).unwrap();
        f.files = Files::workspace(
            ws,
            Arc::new(IgnoreRules::new(&[], true)),
            Arc::new(HashCache::new()),
            Limits {
                max_blob_bytes: 4,
                ..Limits::default()
            },
            Arc::new(GateState::new(Gates::OPEN)),
        );
        std::fs::create_dir(f.dir.join("source")).unwrap();
        std::fs::write(f.dir.join("source/a"), "12").unwrap();
        std::fs::write(f.dir.join("source/b"), "34").unwrap();
        f.files.copy(&path("source"), &path("fits")).unwrap();
        assert_eq!(read(&f.dir, "fits/a"), "12");
        assert_eq!(read(&f.dir, "fits/b"), "34");

        std::fs::write(f.dir.join("source/b"), "345").unwrap();
        assert!(matches!(
            f.files.copy(&path("source"), &path("too-large")),
            Err(HostError::TooLarge { limit: 4, .. })
        ));
        assert!(!f.dir.join("too-large").exists());
        let names: Vec<_> = std::fs::read_dir(&f.dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            names
                .iter()
                .all(|name| !name.to_string_lossy().starts_with(TEMP_PREFIX))
        );
    }

    #[test]
    fn a_copy_leaves_out_what_the_ignore_rules_leave_out() {
        let f = fixture_with(&["*.o"], Gates::OPEN);
        std::fs::create_dir_all(f.dir.join("proj")).unwrap();
        std::fs::write(f.dir.join("proj/main.c"), "c").unwrap();
        std::fs::write(f.dir.join("proj/main.o"), "o").unwrap();
        f.files.copy(&path("proj"), &path("proj2")).unwrap();
        assert!(f.dir.join("proj2/main.c").exists());
        assert!(!f.dir.join("proj2/main.o").exists());
    }

    #[test]
    fn removing_a_directory_needs_recursive_and_a_link_is_removed_without_touching_what_it_leads_to()
     {
        let f = fixture();
        std::fs::create_dir_all(f.dir.join("d/e")).unwrap();
        std::fs::write(f.dir.join("d/e/x"), "x").unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("keep"), "keep").unwrap();
        std::os::unix::fs::symlink(outside.path(), f.dir.join("d/out")).unwrap();
        assert!(matches!(
            f.files.remove(&path("d"), false),
            Err(HostError::Exists(_))
        ));
        f.files.remove(&path("d"), true).unwrap();
        assert!(!f.dir.join("d").exists());
        assert_eq!(
            read(outside.path(), "keep"),
            "keep",
            "a recursive removal does not follow a link out"
        );
        assert!(matches!(
            f.files.remove(&RelPath::root(), true),
            Err(HostError::Unsupported(_))
        ));
        assert!(matches!(
            f.files.remove(&path("d"), true),
            Err(HostError::NotFound(_))
        ));
    }

    #[test]
    fn a_path_out_of_the_workspace_is_refused_by_every_operation() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "secret").unwrap();
        let f = fixture();
        std::os::unix::fs::symlink(outside.path(), f.dir.join("out")).unwrap();
        std::fs::write(f.dir.join("inside"), "inside").unwrap();
        let blob = f.blobs.insert(&b"x"[..]);
        assert!(matches!(
            f.files.list(&path("out")),
            Err(HostError::NotFound(_))
        ));
        assert!(matches!(
            f.files.open_regular(&path("out/secret")),
            Err(HostError::NotFound(_))
        ));
        assert!(matches!(
            f.files.stat(&path("out/secret")),
            Err(HostError::NotFound(_))
        ));
        assert!(matches!(
            block_on(
                f.files
                    .write(&path("out/secret"), blob, None, None, &f.blobs)
            ),
            Err(HostError::NotFound(_))
        ));
        assert!(matches!(
            block_on(f.files.write(&path("out/new"), blob, None, None, &f.blobs)),
            Err(HostError::NotFound(_))
        ));
        assert!(
            f.files
                .rename(&path("inside"), &path("out/secret2"))
                .is_err()
        );
        assert!(f.files.copy(&path("inside"), &path("out/copy")).is_err());
        assert!(f.files.copy(&path("out/secret"), &path("stolen")).is_err());
        assert!(f.files.remove(&path("out/secret"), false).is_err());
        assert!(f.files.make_dir(&path("out/newdir")).is_err());
        assert!(RelPath::parse("../x", PathStyle::Posix).is_err());
        assert_eq!(read(outside.path(), "secret"), "secret");
        assert!(!outside.path().join("new").exists() && !outside.path().join("secret2").exists());
        assert!(!f.dir.join("stolen").exists());
    }

    #[test]
    fn a_mount_is_listed_and_read_without_ignore_rules_and_never_written() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("data.csv"), "a,b\n").unwrap();
        std::fs::write(root.path().join("debug.log"), "x").unwrap();
        let ws = Workspace::open(root.path(), &[], PathStyle::Posix, true).unwrap();
        let mount = Files::mount(
            ws,
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(Gates::OPEN)),
        );
        let names: Vec<String> = mount
            .list(&RelPath::root())
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["data.csv", "debug.log"]);
        assert!(matches!(
            mount.make_dir(&path("new")),
            Err(HostError::Unsupported(_))
        ));
        assert!(matches!(
            mount.remove(&path("data.csv"), false),
            Err(HostError::Unsupported(_))
        ));
    }
}
