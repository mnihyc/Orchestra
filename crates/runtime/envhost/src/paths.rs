//! Containment in the workspace root, symlinks, case sensitivity and portable path rules.
//!
//! Every path Orch exposes is relative to the workspace root: `/`-separated, with no empty, `.` or
//! `..` component and at most [`PATH_MAX_BYTES`] bytes. [`RelPath`] is such a path and cannot be
//! made from anything else, so a path that has been parsed has already been refused for every
//! way a *spelling* can leave the root. [`Workspace`] is the root itself, held open: it finds
//! where a path really is by walking it one directory at a time and reading each link on the way,
//! and refuses a link whose target lies outside the root, however many links lead there. What it
//! hands back is the directory that holds the entry and the entry's name, so that what the
//! caller then does can only land inside that directory, even when somebody replaces a directory
//! with a link in the meantime.
//!
//! The rules for Windows names (reserved device names, characters, trailing dots) are applied
//! where the environment writes its paths that way; on other systems a name is whatever the file
//! system allows.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

pub use orch_link_contract::events::PathStyle;

use crate::error::{HostError, Result};
use crate::platform::{Dir, Kind, Meta};

/// The longest path Orch accepts, in bytes.
pub const PATH_MAX_BYTES: usize = 4096;

/// How many links one path may lead through before it is given up. The kernel's own limit is the
/// same, so a path that works for a program there works here.
pub const SYMLINK_HOPS_MAX: u32 = 40;

/// The directory that is Orch's own inside a thread's directory.
pub const ORCH_DIR: &str = ".orch";

/// What the name of a file the host is still writing begins with. Such a file is never shown, never
/// walked and never hashed: it is either finished and renamed to its real name, or removed.
pub const TEMP_PREFIX: &str = ".orch-tmp-";

/// Whether a name is control metadata or one of the host's own half-written files.
#[must_use]
pub fn is_internal_name(name: &str) -> bool {
    name == orch_agent_contract::session::INSTRUCTION_ROOT_MARKER
        || name.starts_with(TEMP_PREFIX)
        || name.starts_with(".orch-case-probe-")
}

/// A worktree's permanent Git administration keeps the basename of its original owned stage.
pub(crate) fn is_internal_entry(parent: &RelPath, name: &str) -> bool {
    let worktree_metadata = parent.file_name() == Some("worktrees")
        && parent
            .parent()
            .is_some_and(|path| path.file_name() == Some(".git"))
        && name
            .strip_prefix(".orch-tmp-worktree-")
            .is_some_and(|id| id.parse::<orch_types::Id>().is_ok());
    is_internal_name(name) && !worktree_metadata
}

/// Why a text is not a path of the workspace.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathFault {
    /// More than [`PATH_MAX_BYTES`] bytes.
    #[error("a path has at most {PATH_MAX_BYTES} bytes")]
    TooLong,
    /// A NUL byte.
    #[error("a path has no NUL byte")]
    Nul,
    /// It begins with `/`: paths are relative to the workspace root.
    #[error("a path is relative to the workspace root and does not begin with /")]
    Absolute,
    /// Two separators in a row, or one at the end.
    #[error("a path has no empty component")]
    EmptyComponent,
    /// A `.` component.
    #[error("a path has no . component")]
    DotComponent,
    /// A `..` component.
    #[error("a path has no .. component")]
    ParentComponent,
    /// A name Windows keeps for a device.
    #[error("{0:?} is a name Windows keeps for a device")]
    ReservedName(String),
    /// A character a Windows name cannot hold.
    #[error("{0:?} holds a character that a Windows name cannot")]
    ForbiddenCharacter(String),
    /// A name that ends in a dot or a space, which Windows drops.
    #[error("{0:?} ends in a dot or a space, which Windows drops")]
    TrailingDotOrSpace(String),
}

impl From<PathFault> for HostError {
    fn from(fault: PathFault) -> Self {
        // A path that is not one is a path to nothing; it is answered as everything else outside the root is.
        Self::NotFound(fault.to_string())
    }
}

/// A path of the workspace, relative to its root. The empty path is the root.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct RelPath(String);

impl std::fmt::Display for RelPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The characters a Windows file name cannot hold, besides the control characters.
const WINDOWS_FORBIDDEN: &[char] = &['<', '>', ':', '"', '|', '?', '*', '\\'];

/// The names Windows keeps for devices, whatever their extension.
const WINDOWS_DEVICE_NAMES: &[&str] = &["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "CLOCK$"];

fn windows_reserved(name: &str) -> bool {
    // "CON.txt" and "con .txt" are the device too: the name is what comes before the first dot,
    // without the spaces that end it.
    let base = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    let upper = base.to_uppercase();
    if WINDOWS_DEVICE_NAMES.contains(&upper.as_str()) {
        return true;
    }
    for prefix in ["COM", "LPT"] {
        if let Some(rest) = upper.strip_prefix(prefix) {
            let mut digits = rest.chars();
            if let (Some(digit), None) = (digits.next(), digits.next())
                && (('1'..='9').contains(&digit) || matches!(digit, '\u{b9}' | '\u{b2}' | '\u{b3}'))
            {
                return true;
            }
        }
    }
    false
}

fn check_windows_name(name: &str) -> std::result::Result<(), PathFault> {
    if name
        .chars()
        .any(|c| c.is_control() || WINDOWS_FORBIDDEN.contains(&c))
    {
        return Err(PathFault::ForbiddenCharacter(name.to_owned()));
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(PathFault::TrailingDotOrSpace(name.to_owned()));
    }
    if windows_reserved(name) {
        return Err(PathFault::ReservedName(name.to_owned()));
    }
    Ok(())
}

impl RelPath {
    /// The root.
    #[must_use]
    pub fn root() -> Self {
        Self(String::new())
    }

    /// The path `text` says, written as `style` writes paths. Windows separators are converted
    /// where the environment is a Windows one; elsewhere a backslash is part of a name.
    pub fn parse(text: &str, style: PathStyle) -> std::result::Result<Self, PathFault> {
        let converted;
        let text = if style == PathStyle::Windows && text.contains('\\') {
            converted = text.replace('\\', "/");
            converted.as_str()
        } else {
            text
        };
        if text.len() > PATH_MAX_BYTES {
            return Err(PathFault::TooLong);
        }
        if text.contains('\0') {
            return Err(PathFault::Nul);
        }
        if text.is_empty() {
            return Ok(Self::root());
        }
        if text.starts_with('/') {
            return Err(PathFault::Absolute);
        }
        for component in text.split('/') {
            match component {
                "" => return Err(PathFault::EmptyComponent),
                "." => return Err(PathFault::DotComponent),
                ".." => return Err(PathFault::ParentComponent),
                name if style == PathStyle::Windows => check_windows_name(name)?,
                _ => {}
            }
        }
        Ok(Self(text.to_owned()))
    }

    /// A path that came out of a walk of the file system or out of this package's own storage of
    /// one, and so has been a path of the workspace already.
    pub(crate) fn from_walked(text: String) -> Self {
        Self(text)
    }

    /// The path as it is written: `/`-separated, empty for the root.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is the root.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The names of the path, from the root down.
    pub fn components(&self) -> impl DoubleEndedIterator<Item = &str> {
        self.0.split('/').filter(|component| !component.is_empty())
    }

    /// How many names the path has.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.components().count()
    }

    /// The last name; `None` for the root.
    #[must_use]
    pub fn file_name(&self) -> Option<&str> {
        self.components().next_back()
    }

    /// The path without its last name; `None` for the root.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            return None;
        }
        Some(Self(
            self.0
                .rsplit_once('/')
                .map_or_else(String::new, |(parent, _)| parent.to_owned()),
        ))
    }

    /// This path with one more name. The name is a plain name of one entry: a name that holds a
    /// separator is not accepted, so the result is always a path of the workspace.
    pub fn join(&self, name: &str) -> Result<Self> {
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\0')
        {
            return Err(HostError::NotFound(format!("{name:?} is not a name")));
        }
        if self.is_root() {
            Ok(Self(name.to_owned()))
        } else {
            Ok(Self(format!("{}/{name}", self.0)))
        }
    }

    /// Appends a checked relative path one component at a time, keeping join's single-name rule.
    pub fn append(&self, suffix: &RelPath) -> Result<Self> {
        suffix
            .components()
            .try_fold(self.clone(), |path, name| path.join(name))
    }

    /// Whether `other` is this path or lies above it, name by name.
    #[must_use]
    pub fn starts_with(&self, other: &RelPath) -> bool {
        other.is_root()
            || self.0 == other.0
            || self
                .0
                .strip_prefix(other.0.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    }

    /// What is left of this path below `base`, when it lies below it.
    #[must_use]
    pub fn strip_prefix(&self, base: &RelPath) -> Option<RelPath> {
        if base.is_root() {
            return Some(self.clone());
        }
        if self.0 == base.0 {
            return Some(Self::root());
        }
        self.0
            .strip_prefix(base.0.as_str())
            .and_then(|rest| rest.strip_prefix('/'))
            .map(|rest| Self(rest.to_owned()))
    }

    /// Whether a name of the path is `name`.
    #[must_use]
    pub fn has_component(&self, name: &str) -> bool {
        self.components().any(|component| component == name)
    }

    /// Whether this path and `other` are the same path in a file system that does, or does not,
    /// tell upper and lower case apart.
    #[must_use]
    pub fn same_as(&self, other: &RelPath, case_sensitive: bool) -> bool {
        if case_sensitive {
            self.0 == other.0
        } else {
            self.0.to_lowercase() == other.0.to_lowercase()
        }
    }

    /// Whether the path can be made on any of the systems a workspace may move between: no
    /// name Windows would refuse. A snapshot taken on Linux can only be unpacked on Windows as far
    /// as this holds.
    #[must_use]
    pub fn is_portable(&self) -> bool {
        self.components()
            .all(|name| check_windows_name(name).is_ok())
    }

    /// The path below `base`, as a path of the file system.
    #[must_use]
    pub fn under(&self, base: &Path) -> PathBuf {
        let mut path = base.to_path_buf();
        for component in self.components() {
            path.push(component);
        }
        path
    }
}

/// Whether links in the last name of a path are followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Follow {
    /// A link at the end is the entry itself: it is renamed, removed, copied and listed as a link.
    Never,
    /// A link at the end is followed to what it points at: a file is read through it and a
    /// directory is listed through it. Links in the middle of a path are always followed.
    Final,
}

/// What a request for a path leads to.
#[derive(Debug)]
pub struct Located {
    /// The directory that holds the entry.
    pub parent: Dir,
    /// The entry's name there; `None` for the root itself, which has no name.
    pub name: Option<OsString>,
    /// Where the entry really is, with every link on the way read.
    pub real: RelPath,
    /// What it is, without following it; `None` when nothing is there.
    pub meta: Option<Meta>,
}

impl Located {
    /// The entry's name, which the root has not.
    pub fn name(&self) -> Result<&OsStr> {
        self.name.as_deref().ok_or_else(|| {
            HostError::Unsupported("the workspace root cannot be changed".to_owned())
        })
    }

    /// Whether anything is there.
    #[must_use]
    pub fn exists(&self) -> bool {
        self.meta.is_some()
    }
}

/// How a request walks a path.
#[derive(Debug, Clone, Copy)]
pub struct Locate {
    /// Whether a link at the end is followed.
    pub follow: Follow,
    /// Whether directories that are missing on the way are made.
    pub make_parents: bool,
}

impl Locate {
    /// To look at the entry itself.
    pub const ENTRY: Self = Self {
        follow: Follow::Never,
        make_parents: false,
    };
    /// To read through a link at the end.
    pub const THROUGH: Self = Self {
        follow: Follow::Final,
        make_parents: false,
    };
    /// To write through a link at the end, making the directories above it.
    pub const FOR_WRITE: Self = Self {
        follow: Follow::Final,
        make_parents: true,
    };
    /// To make the entry itself (a new name), making the directories above it.
    pub const FOR_CREATE: Self = Self {
        follow: Follow::Never,
        make_parents: true,
    };
}

/// A workspace root, held open, and the rules for walking from it.
#[derive(Debug)]
pub struct Workspace {
    root: Dir,
    root_path: PathBuf,
    aliases: Vec<PathBuf>,
    style: PathStyle,
    case_sensitive: bool,
}

/// One directory of the walk, with the name it was entered by.
struct Frame {
    dir: Dir,
    name: OsString,
}

impl Workspace {
    /// Opens the directory at `root`. Its real path is read once; other spellings of the root
    /// that `aliases` lists are the ones an absolute link inside the workspace may use.
    pub fn open(
        root: &Path,
        aliases: &[PathBuf],
        style: PathStyle,
        case_sensitive: bool,
    ) -> Result<Self> {
        let root_path = std::fs::canonicalize(root).map_err(|error| {
            HostError::io(
                format!("opening the workspace root {}", root.display()),
                error,
            )
        })?;
        let handle = Dir::open_path(&root_path).map_err(|error| {
            HostError::io(
                format!("opening the workspace root {}", root.display()),
                error,
            )
        })?;
        let mut all = vec![root.to_path_buf()];
        all.extend(aliases.iter().cloned());
        all.retain(|path| *path != root_path);
        Ok(Self {
            root: handle,
            root_path,
            aliases: all,
            style,
            case_sensitive,
        })
    }

    /// The real path of the root.
    #[must_use]
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    /// How this workspace writes its paths.
    #[must_use]
    pub fn style(&self) -> PathStyle {
        self.style
    }

    /// Whether names differ by case here.
    #[must_use]
    pub fn case_sensitive(&self) -> bool {
        self.case_sensitive
    }

    /// Reads a path as a request writes it.
    pub fn parse(&self, text: &str) -> Result<RelPath> {
        Ok(RelPath::parse(text, self.style)?)
    }

    /// Another handle on the root, to start a walk from.
    pub fn root_dir(&self) -> Result<Dir> {
        self.root
            .try_clone()
            .map_err(|error| HostError::io("holding the workspace root", error))
    }

    /// What the root itself is.
    pub fn root_meta(&self) -> Result<Meta> {
        self.root
            .stat_self()
            .map_err(|error| HostError::io("reading the workspace root", error))
    }

    /// Where an absolute link points, below the root, when it points below it at all.
    fn below_root(&self, target: &Path) -> Option<PathBuf> {
        std::iter::once(&self.root_path)
            .chain(self.aliases.iter())
            .find_map(|root| target.strip_prefix(root).ok().map(Path::to_path_buf))
    }

    /// Where `absolute`, a path as the machine spells it, lies below the root, when it does at all.
    #[must_use]
    pub fn relative_to_root(&self, absolute: &Path) -> Option<PathBuf> {
        self.below_root(absolute)
    }

    fn real_of(frames: &[Frame], tail: Option<&OsStr>) -> RelPath {
        let mut text = String::new();
        for name in frames
            .iter()
            .skip(1)
            .map(|frame| frame.name.as_os_str())
            .chain(tail)
        {
            if !text.is_empty() {
                text.push('/');
            }
            text.push_str(&name.to_string_lossy());
        }
        RelPath(text)
    }

    /// Walks `path` from the root, reading every link on the way, and says where it leads. A
    /// link whose target lies outside the root, a `..` that would climb out of it, and a chain of
    /// more than [`SYMLINK_HOPS_MAX`] links are all `not_found`.
    pub fn locate(&self, path: &RelPath, how: Locate) -> Result<Located> {
        let io_err =
            |what: &str, error: std::io::Error| HostError::io(format!("{what} {path}"), error);
        let mut frames = vec![Frame {
            dir: self.root_dir()?,
            name: OsString::new(),
        }];
        let mut queue: VecDeque<(OsString, bool)> = {
            let names: Vec<&str> = path.components().collect();
            let count = names.len();
            names
                .into_iter()
                .enumerate()
                .map(|(i, name)| (OsString::from(name), i + 1 == count))
                .collect()
        };
        let mut hops = 0u32;
        while let Some((component, last)) = queue.pop_front() {
            if component == ".." {
                if frames.len() == 1 {
                    return Err(HostError::NotFound(format!("{path} leaves the workspace")));
                }
                frames.pop();
                continue;
            }
            if component == "." {
                continue;
            }
            let top = &frames[frames.len() - 1].dir;
            let meta = match top.lstat(&component) {
                Ok(meta) => meta,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if last {
                        let parent = top
                            .try_clone()
                            .map_err(|e| io_err("holding the parent of", e))?;
                        let real = Self::real_of(&frames, Some(&component));
                        return Ok(Located {
                            parent,
                            name: Some(component),
                            real,
                            meta: None,
                        });
                    }
                    if how.make_parents {
                        top.mkdir(&component, 0o777)
                            .map_err(|e| io_err("making the directories of", e))?;
                        let child = top
                            .open_child(&component)
                            .map_err(|e| io_err("entering", e))?;
                        frames.push(Frame {
                            dir: child,
                            name: component,
                        });
                        continue;
                    }
                    return Err(HostError::NotFound(format!("{path} is not there")));
                }
                Err(error) => return Err(io_err("looking at", error)),
            };
            match meta.kind {
                Kind::Symlink if !last || how.follow == Follow::Final => {
                    hops += 1;
                    if hops > SYMLINK_HOPS_MAX {
                        return Err(HostError::NotFound(format!(
                            "{path} leads through too many links"
                        )));
                    }
                    let target = top
                        .read_link(&component)
                        .map_err(|e| io_err("reading the link in", e))?;
                    let target_path = PathBuf::from(&target);
                    let relative = if target_path.is_absolute() {
                        let below = self.below_root(&target_path).ok_or_else(|| {
                            HostError::NotFound(format!(
                                "a link on the way to {path} points outside the workspace"
                            ))
                        })?;
                        frames.truncate(1);
                        below
                    } else {
                        target_path
                    };
                    let mut expansion: Vec<OsString> = Vec::new();
                    for part in relative.components() {
                        match part {
                            Component::Normal(name) => expansion.push(name.to_os_string()),
                            Component::ParentDir => expansion.push(OsString::from("..")),
                            Component::CurDir | Component::RootDir => {}
                            Component::Prefix(_) => {
                                return Err(HostError::NotFound(format!(
                                    "a link on the way to {path} points outside the workspace"
                                )));
                            }
                        }
                    }
                    let count = expansion.len();
                    for (i, name) in expansion.into_iter().enumerate().rev() {
                        queue.push_front((name, last && i + 1 == count));
                    }
                }
                Kind::Dir if !last => {
                    let child = top
                        .open_child(&component)
                        .map_err(|e| io_err("entering", e))?;
                    frames.push(Frame {
                        dir: child,
                        name: component,
                    });
                }
                _ if !last => {
                    return Err(HostError::NotFound(format!(
                        "{path} goes through something that is not a directory"
                    )));
                }
                _ => {
                    let parent = top
                        .try_clone()
                        .map_err(|e| io_err("holding the parent of", e))?;
                    let real = Self::real_of(&frames, Some(&component));
                    return Ok(Located {
                        parent,
                        name: Some(component),
                        real,
                        meta: Some(meta),
                    });
                }
            }
        }
        // The walk ended on a directory it entered: the root, or what a link or a `..` led to.
        let real = Self::real_of(&frames, None);
        if frames.len() == 1 {
            let meta = self.root_meta()?;
            return Ok(Located {
                parent: self.root_dir()?,
                name: None,
                real,
                meta: Some(meta),
            });
        }
        let name = frames[frames.len() - 1].name.clone();
        let parent = frames[frames.len() - 2]
            .dir
            .try_clone()
            .map_err(|e| io_err("holding the parent of", e))?;
        let meta = parent.lstat(&name).ok();
        Ok(Located {
            parent,
            name: Some(name),
            real,
            meta,
        })
    }

    /// Opens the directory `located` is, or a link in it leads to; anything else is an error.
    pub fn open_dir(&self, located: &Located) -> Result<Dir> {
        match (&located.name, located.meta) {
            (None, _) => self.root_dir(),
            (Some(name), Some(meta)) if meta.is_dir() => located
                .parent
                .open_child(name)
                .map_err(|error| HostError::io(format!("opening {}", located.real), error)),
            (Some(_), Some(_)) => Err(HostError::NotFound(format!(
                "{} is not a directory",
                located.real
            ))),
            (Some(_), None) => Err(HostError::NotFound(format!(
                "{} is not there",
                located.real
            ))),
        }
    }

    /// Whether names differ by case in `dir`. It is found out from what is there without writing
    /// anything: a name that has letters is asked for again with its case swapped, and the same
    /// file coming back means case does not matter. Only a directory that holds no such name gets
    /// a file made in it for the question, and removed. `None` where neither works, and the caller
    /// falls back on the platform's usual answer.
    #[must_use]
    pub fn probe_case_sensitivity(dir: &Dir) -> Option<bool> {
        Self::probe_by_reading(dir).or_else(|| Self::probe_by_writing(dir))
    }

    /// How many names of a directory are tried before reading gives up: the first few letters-bearing
    /// names answer, and a directory of thousands of numbered files is not read through for it.
    const CASE_PROBE_NAMES: usize = 64;

    fn probe_by_reading(dir: &Dir) -> Option<bool> {
        let names = dir.names().ok()?;
        for name in names.iter().take(Self::CASE_PROBE_NAMES) {
            let Some(text) = name.to_str() else { continue };
            let swapped = if text.to_uppercase() != text {
                text.to_uppercase()
            } else if text.to_lowercase() != text {
                text.to_lowercase()
            } else {
                continue;
            };
            let Ok(original) = dir.lstat(name) else {
                continue;
            };
            return match dir.lstat(OsStr::new(&swapped)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(true),
                Err(_) => continue,
                // Two names for one file: case does not matter. Where the system numbers no files
                // a name that is found is taken for the same one.
                Ok(other) if (other.dev, other.ino) == (original.dev, original.ino) => Some(false),
                Ok(_) => Some(true),
            };
        }
        None
    }

    fn probe_by_writing(dir: &Dir) -> Option<bool> {
        let name = format!(".orch-case-probe-{}", orch_types::new_id().simple());
        let file = dir.create_new(OsStr::new(&name), 0o600).ok()?;
        drop(file);
        let answer = match dir.lstat(OsStr::new(&name.to_uppercase())) {
            Ok(_) => Some(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(true),
            Err(_) => None,
        };
        let _ = dir.unlink(OsStr::new(&name));
        answer
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn workspace(dir: &Path) -> Workspace {
        Workspace::open(dir, &[], PathStyle::Posix, true).unwrap()
    }

    fn path(text: &str) -> RelPath {
        RelPath::parse(text, PathStyle::Posix).unwrap()
    }

    #[test]
    fn a_path_cannot_be_spelled_so_that_it_leaves_the_root() {
        for bad in [
            "../x",
            "a/../../x",
            "a/..",
            "..",
            "a/./b",
            "./a",
            "/etc/passwd",
            "a//b",
            "a/",
            "//",
        ] {
            assert!(
                RelPath::parse(bad, PathStyle::Posix).is_err(),
                "{bad:?} must be refused"
            );
        }
        assert_eq!(
            RelPath::parse("a/b.txt", PathStyle::Posix)
                .unwrap()
                .to_string(),
            "a/b.txt"
        );
        assert!(RelPath::parse("", PathStyle::Posix).unwrap().is_root());
    }

    #[test]
    fn a_path_has_at_most_4096_bytes_counted_as_bytes() {
        let longest = "a".repeat(PATH_MAX_BYTES);
        assert!(RelPath::parse(&longest, PathStyle::Posix).is_ok());
        let too_long = "a".repeat(PATH_MAX_BYTES + 1);
        assert_eq!(
            RelPath::parse(&too_long, PathStyle::Posix),
            Err(PathFault::TooLong)
        );
        // Two bytes each: half as many characters already pass the limit.
        let wide = "é".repeat(PATH_MAX_BYTES / 2 + 1);
        assert_eq!(
            RelPath::parse(&wide, PathStyle::Posix),
            Err(PathFault::TooLong)
        );
        assert_eq!(
            RelPath::parse("a\0b", PathStyle::Posix),
            Err(PathFault::Nul)
        );
    }

    #[test]
    fn windows_separators_are_converted_where_the_environment_writes_them_so() {
        assert_eq!(
            RelPath::parse("src\\lib\\mod.rs", PathStyle::Windows)
                .unwrap()
                .as_str(),
            "src/lib/mod.rs"
        );
        for bad in ["..\\..\\windows", "a\\..\\b", "\\share", "a\\\\b"] {
            assert!(
                RelPath::parse(bad, PathStyle::Windows).is_err(),
                "{bad:?} must be refused"
            );
        }
        // Elsewhere a backslash is a letter of a name, and cannot climb anywhere.
        let name = RelPath::parse("..\\x", PathStyle::Posix).unwrap();
        assert_eq!(name.depth(), 1);
    }

    #[test]
    fn windows_reserved_names_are_refused_where_they_would_reach_a_device() {
        for bad in [
            "CON",
            "nul",
            "Aux.txt",
            "a/prn.log",
            "COM1",
            "lpt9.dat",
            "con .txt",
            "COM\u{b9}",
            "CONIN$",
        ] {
            assert!(
                matches!(
                    RelPath::parse(bad, PathStyle::Windows),
                    Err(PathFault::ReservedName(_))
                ),
                "{bad:?} must be refused"
            );
        }
        for bad in ["a:b", "a/b?c", "x*", "q\"r", "t.", "u ", "v<w"] {
            assert!(
                RelPath::parse(bad, PathStyle::Windows).is_err(),
                "{bad:?} must be refused"
            );
        }
        for fine in ["console", "com10", "lpt0", "auxiliary.txt", "a.b.c"] {
            assert!(
                RelPath::parse(fine, PathStyle::Windows).is_ok(),
                "{fine:?} is an ordinary name"
            );
        }
        // Where the environment is not Windows these are files like any other.
        assert!(RelPath::parse("aux.c", PathStyle::Posix).is_ok());
        assert!(!path("aux.c").is_portable());
        assert!(path("src/lib.rs").is_portable());
    }

    #[test]
    fn names_that_differ_only_by_case_are_the_same_path_where_the_file_system_says_so() {
        assert!(path("Src/Main.rs").same_as(&path("src/main.rs"), false));
        assert!(!path("Src/Main.rs").same_as(&path("src/main.rs"), true));
    }

    #[test]
    fn a_link_that_points_outside_the_root_leads_nowhere_however_it_is_reached() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("a")).unwrap();
        symlink(outside.path(), root.path().join("out")).unwrap();
        symlink(
            outside.path().join("secret.txt"),
            root.path().join("a/file"),
        )
        .unwrap();
        symlink("../..", root.path().join("a/up")).unwrap();
        symlink("out", root.path().join("two")).unwrap();
        symlink("two", root.path().join("three")).unwrap();
        let ws = workspace(root.path());
        for bad in [
            "out/secret.txt",
            "out",
            "a/file",
            "a/up/x",
            "a/up",
            "two/secret.txt",
            "three/secret.txt",
        ] {
            let found = ws.locate(&path(bad), Locate::THROUGH);
            assert!(
                matches!(found, Err(HostError::NotFound(_))),
                "{bad} must not lead out: {found:?}"
            );
        }
        // The link itself is an entry of the workspace, and can be looked at, renamed and removed.
        let link = ws.locate(&path("out"), Locate::ENTRY).unwrap();
        assert!(link.meta.unwrap().is_symlink());
    }

    #[test]
    fn a_link_that_stays_inside_the_root_is_read_through() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("data/deep")).unwrap();
        std::fs::write(root.path().join("data/deep/note.txt"), "inside").unwrap();
        symlink("data/deep", root.path().join("short")).unwrap();
        symlink(root.path().join("data"), root.path().join("absolute")).unwrap();
        symlink("../data/deep/note.txt", root.path().join("data/back")).unwrap();
        let ws = workspace(root.path());
        let through = ws.locate(&path("short/note.txt"), Locate::THROUGH).unwrap();
        assert_eq!(through.real.as_str(), "data/deep/note.txt");
        let absolute = ws
            .locate(&path("absolute/deep/note.txt"), Locate::THROUGH)
            .unwrap();
        assert_eq!(absolute.real.as_str(), "data/deep/note.txt");
        let ended_on_link = ws.locate(&path("data/back"), Locate::THROUGH).unwrap();
        assert_eq!(ended_on_link.real.as_str(), "data/deep/note.txt");
        let not_followed = ws.locate(&path("data/back"), Locate::ENTRY).unwrap();
        assert!(not_followed.meta.unwrap().is_symlink());
    }

    #[test]
    fn a_loop_of_links_ends_with_not_found_and_does_not_hang() {
        let root = tempfile::tempdir().unwrap();
        symlink("two", root.path().join("one")).unwrap();
        symlink("one", root.path().join("two")).unwrap();
        let ws = workspace(root.path());
        assert!(matches!(
            ws.locate(&path("one"), Locate::THROUGH),
            Err(HostError::NotFound(_))
        ));
        assert!(matches!(
            ws.locate(&path("one/x"), Locate::THROUGH),
            Err(HostError::NotFound(_))
        ));
    }

    #[test]
    fn a_directory_swapped_for_a_link_after_the_walk_cannot_redirect_what_is_done_with_it() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("file"), "outside").unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("dir")).unwrap();
        std::fs::write(root.path().join("dir/file"), "inside").unwrap();
        let ws = workspace(root.path());
        let located = ws.locate(&path("dir/file"), Locate::THROUGH).unwrap();
        // Between the walk and the use, the directory is replaced by a link to somewhere else.
        std::fs::rename(root.path().join("dir"), root.path().join("moved")).unwrap();
        symlink(outside.path(), root.path().join("dir")).unwrap();
        let mut text = String::new();
        std::io::Read::read_to_string(
            &mut located.parent.open_read(located.name().unwrap()).unwrap(),
            &mut text,
        )
        .unwrap();
        assert_eq!(
            text, "inside",
            "the handle still means the directory that was walked"
        );
    }

    #[test]
    fn missing_directories_are_made_only_when_the_walk_is_to_write() {
        let root = tempfile::tempdir().unwrap();
        let ws = workspace(root.path());
        assert!(matches!(
            ws.locate(&path("a/b/c.txt"), Locate::THROUGH),
            Err(HostError::NotFound(_))
        ));
        let located = ws.locate(&path("a/b/c.txt"), Locate::FOR_WRITE).unwrap();
        assert!(located.meta.is_none());
        assert!(root.path().join("a/b").is_dir());
        // Making them is still held to the root: a link on the way that leaves it stops the walk.
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), root.path().join("out")).unwrap();
        assert!(ws.locate(&path("out/new/file"), Locate::FOR_WRITE).is_err());
        assert!(!outside.path().join("new").exists());
    }

    #[test]
    fn the_root_is_found_by_the_empty_path_and_by_a_link_that_points_back_to_it() {
        let root = tempfile::tempdir().unwrap();
        symlink(".", root.path().join("self")).unwrap();
        let ws = workspace(root.path());
        let root_found = ws.locate(&RelPath::root(), Locate::THROUGH).unwrap();
        assert!(root_found.name.is_none());
        let by_link = ws.locate(&path("self"), Locate::THROUGH).unwrap();
        assert!(by_link.name.is_none(), "a link to . is the root");
        assert!(root_found.name().is_err(), "the root has no name to change");
    }

    #[test]
    fn whether_names_differ_by_case_is_found_by_asking_the_directory() {
        let root = tempfile::tempdir().unwrap();
        let dir = Dir::open_path(root.path()).unwrap();
        let answer = Workspace::probe_case_sensitivity(&dir);
        assert!(answer.is_some());
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            0,
            "the probe removes itself"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_directory_that_holds_a_named_file_is_asked_without_writing_anything_in_it() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("Readme.md"), "x").unwrap();
        let before = std::fs::metadata(root.path()).unwrap().modified().unwrap();
        let dir = Dir::open_path(root.path()).unwrap();
        assert_eq!(
            Workspace::probe_case_sensitivity(&dir),
            Some(true),
            "a temporary directory of Linux tells the two names apart"
        );
        assert_eq!(
            std::fs::metadata(root.path()).unwrap().modified().unwrap(),
            before,
            "nothing was made or removed there"
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
