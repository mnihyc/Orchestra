//! A directory for each thread under the workspace root: where its agent starts and its views open, with `.orch` inside it for what Orch and the agent keep there (uploads, shares, the agent's notes) under an ignore file of its own; and not a boundary.
//!
//! A thread's directory is to the thread what a working directory is to a process: its agent
//! starts there and the files panel opens there. Nothing is enforced by it: the workspace is the
//! one boundary there is, and a path from the root reaches everything in it. Threads that work on
//! the same thing share a directory.
//!
//! Inside a thread's directory Orch keeps `.orch/`: `uploads/`, `shared/` and `handoff/` for what
//! is brought to the agent, and `memory/` for what the agent notes for itself. When Orch writes
//! there it makes sure a `.gitignore` that ignores everything in the directory stands beside the
//! rest, so that a repository around it neither lists nor commits what belongs to the thread and
//! not to the work. `.orch/` is visible in Orch and is never left out by any ignore rule.

use std::ffi::OsStr;

use crate::error::{HostError, Result};
use crate::files::Files;
use crate::paths::{Locate, ORCH_DIR, RelPath};

/// The directory of what is brought to the agent with a message.
pub const UPLOADS_DIR: &str = "uploads";

/// The directory of what is shared with the agent.
pub const SHARED_DIR: &str = "shared";

/// The directory of what is handed over to the agent when a conversation is summarised.
pub const HANDOFF_DIR: &str = "handoff";

/// The directory of what the agent notes for itself.
pub const MEMORY_DIR: &str = "memory";

/// The file that keeps a repository from listing what is in `.orch/`.
pub const GITIGNORE_NAME: &str = ".gitignore";

/// What that file says: everything in the directory, itself included.
pub const GITIGNORE_BODY: &str = "*\n";

/// The bits the file is made with before the umask takes its part.
const GITIGNORE_MODE: u32 = 0o666;

/// A thread's directory as it was made ready.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadDirectory {
    /// The directory, from the workspace root.
    pub path: RelPath,
    /// Its `.orch/`.
    pub orch: RelPath,
}

/// The place inside `directory/.orch/` called `name`.
pub fn orch_path(directory: &RelPath, name: &str) -> Result<RelPath> {
    directory.join(ORCH_DIR)?.join(name)
}

impl Files {
    /// Makes the directory of a thread and its `.orch/` with the file that keeps a repository from
    /// listing it. Directories that are there are left as they are.
    pub fn ensure_thread_directory(&self, directory: &RelPath) -> Result<ThreadDirectory> {
        self.make_dir(directory)?;
        let orch = directory.join(ORCH_DIR)?;
        self.make_dir(&orch)?;
        self.note_orch_write(&orch);
        Ok(ThreadDirectory {
            path: directory.clone(),
            orch,
        })
    }

    /// Notes that Orch wrote at `real`: when that is inside an `.orch/`, the `.gitignore` that
    /// belongs beside the rest of it is made if it is not there. Nothing here is a reason to fail
    /// the write that was done.
    pub(crate) fn note_orch_write(&self, real: &RelPath) {
        let mut orch = RelPath::root();
        let mut found = false;
        for name in real.components() {
            match orch.join(name) {
                Ok(next) => orch = next,
                Err(_) => return,
            }
            if name == ORCH_DIR {
                found = true;
                break;
            }
        }
        if !found {
            return;
        }
        let Ok(located) = self.root().locate(&orch, Locate::ENTRY) else {
            return;
        };
        let Ok(dir) = self.root().open_dir(&located) else {
            return;
        };
        let name = OsStr::new(GITIGNORE_NAME);
        if let Ok(file) = dir.create_new(name, GITIGNORE_MODE) {
            use std::io::Write;
            let mut file = file;
            if file.write_all(GITIGNORE_BODY.as_bytes()).is_err() {
                // A half-made file is worse than none: it would stand for the real one.
                let _ = dir.unlink(name);
            }
        }
    }

    /// Whether `path` is `.orch` or lies inside one.
    #[must_use]
    pub fn is_orch_path(path: &RelPath) -> bool {
        path.has_component(ORCH_DIR)
    }

    /// Refuses a request whose directory is not one a thread can have: it must lie in the workspace
    /// and not inside `.orch/`.
    pub fn check_thread_directory(directory: &RelPath) -> Result<()> {
        if Self::is_orch_path(directory) {
            return Err(HostError::Unsupported(
                "a thread's directory is not inside .orch/".to_owned(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::{GateState, Gates, Limits};
    use crate::ignore::IgnoreRules;
    use crate::index::HashCache;
    use crate::paths::{PathStyle, Workspace};

    fn native_style() -> PathStyle {
        if cfg!(windows) {
            PathStyle::Windows
        } else {
            PathStyle::Posix
        }
    }
    use std::sync::Arc;

    fn files_in(root: &std::path::Path, patterns: &[&str]) -> Files {
        let ws = Workspace::open(root, &[], native_style(), true).unwrap();
        let rules = Arc::new(IgnoreRules::new(
            &patterns.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>(),
            true,
        ));
        Files::workspace(
            ws,
            rules,
            Arc::new(HashCache::new()),
            Limits::default(),
            Arc::new(GateState::new(Gates::OPEN)),
        )
    }

    fn path(text: &str) -> RelPath {
        RelPath::parse(text, native_style()).unwrap()
    }

    #[test]
    fn a_threads_directory_is_made_with_its_orch_directory_and_the_file_that_keeps_a_repository_out()
     {
        let root = tempfile::tempdir().unwrap();
        let files = files_in(root.path(), &[]);
        let made = files.ensure_thread_directory(&path("web/app")).unwrap();
        assert_eq!(made.orch.as_str(), "web/app/.orch");
        assert_eq!(
            std::fs::read_to_string(root.path().join("web/app/.orch/.gitignore")).unwrap(),
            "*\n"
        );
        // Made again, it stays as it is, and a file somebody put in is not touched.
        std::fs::write(root.path().join("web/app/.orch/note"), "mine").unwrap();
        files.ensure_thread_directory(&path("web/app")).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("web/app/.orch/note")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn a_repository_around_the_directory_does_not_list_what_orch_keeps_there() {
        let root = tempfile::tempdir().unwrap();
        let files = files_in(root.path(), &[]);
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(root.path())
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        git(&["init", "-q"]);
        files.ensure_thread_directory(&path("work")).unwrap();
        std::fs::write(root.path().join("work/.orch/uploads-note"), "x").unwrap();
        std::fs::write(root.path().join("work/code.rs"), "fn main() {}").unwrap();
        let status = git(&["status", "--porcelain", "-uall"]);
        assert!(status.contains("work/code.rs"), "{status}");
        assert!(
            !status.contains(".orch"),
            "what belongs to the thread is not listed: {status}"
        );
    }

    #[test]
    fn a_write_inside_orch_brings_the_ignore_file_with_it_and_orch_is_never_left_out() {
        let root = tempfile::tempdir().unwrap();
        // Every pattern there is: Orch's own directory is still seen.
        let files = files_in(root.path(), &["*", ".*"]);
        std::fs::create_dir_all(root.path().join("t/.orch/memory")).unwrap();
        files.note_orch_write(&path("t/.orch/memory/MEMORY.md"));
        assert!(root.path().join("t/.orch/.gitignore").exists());
        std::fs::write(root.path().join("t/.orch/memory/MEMORY.md"), "- note").unwrap();
        let listing = files.list(&path("t/.orch/memory")).unwrap();
        assert_eq!(
            listing.entries.len(),
            1,
            "Orch shows its own directory whatever a pattern says"
        );
        // A path outside any .orch/ makes nothing.
        files.note_orch_write(&path("t/plain.txt"));
        assert!(!root.path().join("t/.gitignore").exists());
    }

    #[test]
    fn the_directory_of_a_thread_is_not_a_boundary_a_path_from_the_root_reaches_the_others() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("one")).unwrap();
        std::fs::create_dir_all(root.path().join("two")).unwrap();
        std::fs::write(root.path().join("two/file.txt"), "other thread's file").unwrap();
        let files = files_in(root.path(), &[]);
        files.ensure_thread_directory(&path("one")).unwrap();
        assert!(files.stat(&path("two/file.txt")).is_ok());
        assert!(Files::check_thread_directory(&path("one/.orch/memory")).is_err());
    }
}
