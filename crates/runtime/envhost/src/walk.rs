//! Walking a tree of the workspace without following links, skipping what the ignore rules leave out.
//!
//! Every part that looks at a whole tree (the index, a manifest, a snapshot, a search, a copy)
//! walks it the same way: directory by directory through handles, never into a link, names in byte
//! order so that two walks of one tree agree, nothing that the ignore rules leave out, and none of
//! the host's own half-written files. The walk hands each entry to the caller together with the
//! directory that holds it, so that the caller opens a file through that handle and not through
//! a path that could have been changed since.

use std::ffi::{OsStr, OsString};
use std::sync::Arc;

use crate::error::{HostError, Result};
use crate::ignore::{IgnoreRules, Scope};
use crate::paths::{Locate, PATH_MAX_BYTES, RelPath, Workspace, is_internal_entry};
use crate::platform::{Dir, Meta};

/// How many directories deep a walk goes. A tree this deep is not made by people; below it the
/// walk stops rather than hold a handle for every level, which would run a process out of them.
pub const WALK_DEPTH_MAX: usize = 256;

/// One entry the walk found.
#[derive(Debug)]
pub struct Entry<'a> {
    /// Where it is, from the workspace root.
    pub path: RelPath,
    /// Its name in its directory.
    pub name: &'a OsStr,
    /// The directory that holds it, open.
    pub parent: &'a Dir,
    /// What it is, without following it.
    pub meta: Meta,
    /// For a link, where it points as written.
    pub link_target: Option<String>,
}

/// What the caller wants the walk to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Go on, into the directory this entry is if it is one.
    Continue,
    /// Do not enter this entry, if it is a directory.
    SkipDir,
    /// Stop the walk.
    Stop,
}

/// How a walk ended.
#[derive(Debug, Default)]
pub struct Outcome {
    /// How many entries were handed to the caller.
    pub visited: u64,
    /// Whether the caller stopped it.
    pub stopped: bool,
    /// Directories and entries the walk could not read, and so did not see.
    pub unreadable: Vec<RelPath>,
}

struct Level {
    dir: Dir,
    scope: Option<Scope>,
    path: RelPath,
    names: std::vec::IntoIter<OsString>,
}

fn sorted_names(dir: &Dir) -> std::io::Result<std::vec::IntoIter<OsString>> {
    let mut names = dir.names()?;
    names.sort();
    Ok(names.into_iter())
}

/// Walks what is below the directory `start` (a real path, with no link in it), handing each entry
/// to `visit`. With `ignore` set, what the rules leave out is not seen; mounts are walked without.
pub fn walk(
    workspace: &Workspace,
    ignore: Option<&Arc<IgnoreRules>>,
    start: &RelPath,
    visit: &mut dyn FnMut(&Entry<'_>) -> Flow,
) -> Result<Outcome> {
    let located = workspace.locate(start, Locate::ENTRY)?;
    let dir = workspace.open_dir(&located)?;
    let scope = match ignore {
        Some(rules) => match rules.scope_at(workspace, start) {
            Some(scope) => Some(scope),
            None => return Ok(Outcome::default()),
        },
        None => None,
    };
    let names =
        sorted_names(&dir).map_err(|error| HostError::io(format!("listing {start}"), error))?;
    let mut stack = vec![Level {
        dir,
        scope,
        path: start.clone(),
        names,
    }];
    let mut outcome = Outcome::default();
    'walk: while let Some(level) = stack.last_mut() {
        crate::error::check_cancelled()?;
        let Some(name) = level.names.next() else {
            stack.pop();
            continue;
        };
        let Some(text) = name.to_str() else { continue };
        if is_internal_entry(&level.path, text) {
            continue;
        }
        let path = match level.path.join(text) {
            Ok(path) if path.as_str().len() <= PATH_MAX_BYTES => path,
            _ => continue,
        };
        let meta = match level.dir.lstat(&name) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                outcome.unreadable.push(path);
                continue;
            }
        };
        if let Some(scope) = &level.scope
            && scope.is_ignored(&path, meta.is_dir())
        {
            continue;
        }
        let link_target = if meta.is_symlink() {
            level
                .dir
                .read_link(&name)
                .ok()
                .map(|target| target.to_string_lossy().into_owned())
        } else {
            None
        };
        outcome.visited += 1;
        let flow = visit(&Entry {
            path: path.clone(),
            name: &name,
            parent: &level.dir,
            meta,
            link_target,
        });
        match flow {
            Flow::Stop => {
                outcome.stopped = true;
                break 'walk;
            }
            Flow::SkipDir => continue,
            Flow::Continue => {}
        }
        if !meta.is_dir() {
            continue;
        }
        if stack.len() >= WALK_DEPTH_MAX {
            outcome.unreadable.push(path);
            continue;
        }
        let opened = stack.last().map(|level| level.dir.open_child(&name));
        match opened {
            Some(Ok(child)) => {
                let scope = stack
                    .last()
                    .and_then(|level| level.scope.as_ref())
                    .map(|scope| scope.enter(&path, &child));
                match sorted_names(&child) {
                    Ok(names) => stack.push(Level {
                        dir: child,
                        scope,
                        path,
                        names,
                    }),
                    Err(_) => outcome.unreadable.push(path),
                }
            }
            _ => outcome.unreadable.push(path),
        }
    }
    Ok(outcome)
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::paths::PathStyle;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    fn workspace(root: &Path) -> Workspace {
        Workspace::open(root, &[], PathStyle::Posix, true).unwrap()
    }

    fn collect(
        workspace: &Workspace,
        ignore: Option<&Arc<IgnoreRules>>,
        start: &str,
    ) -> Vec<String> {
        let mut seen = Vec::new();
        let start = RelPath::parse(start, PathStyle::Posix).unwrap();
        walk(workspace, ignore, &start, &mut |entry| {
            seen.push(entry.path.to_string());
            Flow::Continue
        })
        .unwrap();
        seen
    }

    #[test]
    fn a_walk_goes_down_every_directory_in_byte_order_and_never_into_a_link() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("b/inner")).unwrap();
        std::fs::create_dir_all(root.path().join("a")).unwrap();
        std::fs::write(root.path().join("b/inner/f.txt"), "x").unwrap();
        std::fs::write(root.path().join("a/z"), "x").unwrap();
        std::fs::write(root.path().join("c.txt"), "x").unwrap();
        std::os::unix::fs::symlink("b", root.path().join("to-b")).unwrap();
        let ws = workspace(root.path());
        assert_eq!(
            collect(&ws, None, ""),
            ["a", "a/z", "b", "b/inner", "b/inner/f.txt", "c.txt", "to-b"],
            "the link is an entry and its target is not walked a second time"
        );
        assert_eq!(collect(&ws, None, "b"), ["b/inner", "b/inner/f.txt"]);
    }

    #[test]
    fn what_the_rules_leave_out_is_not_seen_and_not_entered() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src")).unwrap();
        std::fs::create_dir_all(root.path().join("target/debug")).unwrap();
        std::fs::write(root.path().join("src/main.rs"), "x").unwrap();
        std::fs::write(root.path().join("src/.orch_ignore"), "*.bak\n").unwrap();
        std::fs::write(root.path().join("src/main.rs.bak"), "x").unwrap();
        std::fs::write(root.path().join("target/debug/out"), "x").unwrap();
        let ws = workspace(root.path());
        let rules = Arc::new(IgnoreRules::new(&["target/".to_owned()], true));
        assert_eq!(
            collect(&ws, Some(&rules), ""),
            ["src", "src/.orch_ignore", "src/main.rs"]
        );
        // A walk that starts inside a directory that is left out sees nothing at all.
        assert!(collect(&ws, Some(&rules), "target").is_empty());
        assert!(collect(&ws, Some(&rules), "target/debug").is_empty());
    }

    #[test]
    fn the_hosts_own_half_written_files_and_names_that_are_no_text_are_not_seen() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".orch-tmp-1234"), "half").unwrap();
        std::fs::write(root.path().join("fine.txt"), "x").unwrap();
        std::fs::write(
            root.path()
                .join(std::ffi::OsStr::from_bytes(b"bad-\xff-name")),
            "x",
        )
        .unwrap();
        let ws = workspace(root.path());
        assert_eq!(collect(&ws, None, ""), ["fine.txt"]);
    }

    #[test]
    fn the_caller_can_skip_a_directory_or_stop_the_walk() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("skip/deep")).unwrap();
        std::fs::create_dir_all(root.path().join("keep")).unwrap();
        std::fs::write(root.path().join("keep/a"), "x").unwrap();
        std::fs::write(root.path().join("keep/b"), "x").unwrap();
        let ws = workspace(root.path());
        let mut seen = Vec::new();
        walk(&ws, None, &RelPath::root(), &mut |entry| {
            seen.push(entry.path.to_string());
            if entry.path.as_str() == "skip" {
                Flow::SkipDir
            } else {
                Flow::Continue
            }
        })
        .unwrap();
        assert_eq!(seen, ["keep", "keep/a", "keep/b", "skip"]);
        let mut count = 0;
        let outcome = walk(&ws, None, &RelPath::root(), &mut |_| {
            count += 1;
            if count == 2 {
                Flow::Stop
            } else {
                Flow::Continue
            }
        })
        .unwrap();
        assert!(outcome.stopped);
        assert_eq!(outcome.visited, 2);
    }
}
