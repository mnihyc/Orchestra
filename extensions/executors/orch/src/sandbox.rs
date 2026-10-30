//! Which paths a session may reach: the thread's directory and the attached mounts, held by the agent's sandbox mode.
//!
//! With the mode `workspace` or `read_only` the file tools refuse any path that does not lie in the thread's
//! directory or in a mount that is attached, and refuse to write where the mode (`read_only`) or a read-only
//! mount forbids it. With `open` nothing is refused here: what the environment allows, the agent may do.
//!
//! A path is judged by where it really is, not by how it is written: `..` is resolved, and so is every
//! symbolic link on the way, including a link that points nowhere yet (a write through it would create the
//! target). A path that does not exist yet is judged by its nearest existing parent. The check and the use are
//! two steps, so a link swapped in between could still win; a shell command, which this executor does not
//! confine, could do the same without any trick, so this guards the file tools against a mistaken or misled
//! path and is not a boundary against a determined agent. What holds a command in is the environment.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use orch_agent_contract::descriptor::SandboxMode;

use crate::scope::SessionScope;

/// How many symbolic links one path may lead through before it is given up on, as a system's own limit
/// for the same thing is about this many.
const MAX_LINK_HOPS: usize = 40;

/// Why a path, or a command, is refused by the sandbox.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// The path is not in the thread's directory or in an attached mount.
    #[error(
        "{path} is outside what this session may reach. With the sandbox mode {mode} the tools reach {reach}"
    )]
    Outside {
        /// The path as it was resolved.
        path: String,
        /// The sandbox mode.
        mode: &'static str,
        /// The places that may be reached.
        reach: String,
    },
    /// The mode is `read_only`.
    #[error("the sandbox mode is read_only: nothing may be written, and {path} was to be")]
    ReadOnlySession {
        /// The path.
        path: String,
    },
    /// The path is in a mount that nothing may write.
    #[error("{path} is in {holder}, which is read-only: nothing may be written there")]
    ReadOnlyMount {
        /// The path.
        path: String,
        /// The mount, by name.
        holder: String,
    },
    /// A command was to be run with the mode `read_only`.
    #[error(
        "the sandbox mode is read_only and a command cannot be held to reading by this executor, so no command is run; read, list and search are available"
    )]
    NoCommands,
    /// The path cannot be resolved.
    #[error("{path} cannot be resolved: {reason}")]
    Unresolvable {
        /// The path.
        path: String,
        /// What the system said.
        reason: String,
    },
}

impl Refusal {
    /// The code the result carries.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Outside { .. } => "outside_sandbox",
            Self::ReadOnlySession { .. } | Self::ReadOnlyMount { .. } | Self::NoCommands => {
                "read_only"
            }
            Self::Unresolvable { .. } => "invalid_path",
        }
    }
}

/// What is to be done to a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// It is read, listed or searched.
    Read,
    /// It is created, replaced or changed.
    Write,
}

/// One place a session may reach.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Root {
    /// Where it is, as the system resolves it.
    path: PathBuf,
    /// How it is called in a refusal.
    name: String,
    /// Whether nothing may write there.
    read_only: bool,
    /// Whether it is a place to work in (the directory and the mounts), as against a place that is only
    /// read from (the kept output of commands).
    workplace: bool,
}

/// What one session may reach, and in which mode.
#[derive(Debug, Clone)]
pub struct Access {
    mode: SandboxMode,
    roots: Vec<Root>,
}

impl Access {
    /// The reach of the session `scope` describes. `readable` are further places that may be read and
    /// never written: where the output of the thread's commands is kept.
    #[must_use]
    pub fn new(scope: &SessionScope, readable: &[PathBuf]) -> Self {
        let mut roots = vec![Root {
            path: settle(Path::new(&scope.directory)),
            name: "the thread's directory".to_owned(),
            read_only: false,
            workplace: true,
        }];
        roots.extend(scope.mounts.iter().map(|mount| Root {
            path: settle(Path::new(&mount.path)),
            name: format!("the mount {}", mount.name),
            read_only: mount.read_only,
            workplace: true,
        }));
        roots.extend(readable.iter().map(|path| Root {
            path: settle(path),
            name: "the kept output of commands".to_owned(),
            read_only: true,
            workplace: false,
        }));
        Self {
            mode: scope.sandbox_mode,
            roots,
        }
    }

    /// The sandbox mode.
    #[must_use]
    pub const fn mode(&self) -> SandboxMode {
        self.mode
    }

    /// `path` made absolute against `base` and tidied, without asking the file system anything.
    #[must_use]
    pub fn absolute(path: &str, base: &Path) -> PathBuf {
        let given = Path::new(path);
        if given.is_absolute() {
            normalize(given)
        } else {
            normalize(&base.join(given))
        }
    }

    /// The path to use for `path` when `need` is to be done to it, or why not. With the mode `open` it is
    /// the path as it is; otherwise it is the real path, with every link resolved, and it has to lie in a
    /// place the session may reach and, to be written, in one that may be written.
    pub fn permit(&self, path: &Path, need: Need) -> Result<PathBuf, Refusal> {
        if self.mode == SandboxMode::Open {
            // Nothing is refused, but a link is still followed, so that a write goes to the file the link
            // names and does not replace the link.
            return Ok(resolve_real(path).unwrap_or_else(|_| path.to_path_buf()));
        }
        let real = resolve_real(path).map_err(|source| Refusal::Unresolvable {
            path: path.display().to_string(),
            reason: source.to_string(),
        })?;
        let holder = self
            .roots
            .iter()
            .filter(|root| real.starts_with(&root.path))
            .max_by_key(|root| root.path.components().count());
        let Some(holder) = holder else {
            return Err(Refusal::Outside {
                path: real.display().to_string(),
                mode: mode_name(self.mode),
                reach: self.reach(),
            });
        };
        if need == Need::Write {
            if self.mode == SandboxMode::ReadOnly {
                return Err(Refusal::ReadOnlySession {
                    path: real.display().to_string(),
                });
            }
            if holder.read_only {
                return Err(Refusal::ReadOnlyMount {
                    path: real.display().to_string(),
                    holder: holder.name.clone(),
                });
            }
        }
        Ok(real)
    }

    /// Whether a command may be run at all: not with the mode `read_only`, since a command cannot be held
    /// to reading here.
    pub fn permit_commands(&self) -> Result<(), Refusal> {
        if self.mode == SandboxMode::ReadOnly {
            Err(Refusal::NoCommands)
        } else {
            Ok(())
        }
    }

    /// Whether `path`, as it really is, lies in the thread's directory or in an attached mount, whatever
    /// the mode. The directory a `cd` leaves is kept only then.
    #[must_use]
    pub fn in_reach_of_the_thread(&self, path: &Path) -> bool {
        let Ok(real) = resolve_real(path) else {
            return false;
        };
        self.roots
            .iter()
            .any(|root| root.workplace && real.starts_with(&root.path))
    }

    /// The places that may be reached, for a sentence.
    fn reach(&self) -> String {
        self.roots
            .iter()
            .map(|root| {
                let suffix = if root.read_only { ", read-only" } else { "" };
                format!("{} ({}{suffix})", root.path.display(), root.name)
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// The sandbox mode as it is written.
const fn mode_name(mode: SandboxMode) -> &'static str {
    match mode {
        SandboxMode::ReadOnly => "read_only",
        SandboxMode::Workspace => "workspace",
        SandboxMode::Open => "open",
    }
}

/// A place as the system resolves it, or as it is written when it cannot be resolved (it may not exist
/// yet).
fn settle(path: &Path) -> PathBuf {
    resolve_real(&normalize(path)).unwrap_or_else(|_| normalize(path))
}

/// `path` without `.` and `..`, by its text alone. A `..` at the root stays at the root.
#[must_use]
pub fn normalize(path: &Path) -> PathBuf {
    let mut tidy = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => tidy.push(prefix.as_os_str()),
            Component::RootDir => tidy.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !tidy.pop() {
                    tidy.push(Component::ParentDir.as_os_str());
                }
            }
            Component::Normal(name) => tidy.push(name),
        }
    }
    tidy
}

/// Where `path` really is: every link resolved, including one that points nowhere yet, and the part of
/// the path that does not exist yet appended to the real place of its nearest existing parent.
fn resolve_real(path: &Path) -> std::io::Result<PathBuf> {
    let mut current = normalize(path);
    // The parts below the nearest existing parent, last first.
    let mut below: Vec<OsString> = Vec::new();
    for _ in 0..MAX_LINK_HOPS {
        match std::fs::canonicalize(&current) {
            Ok(real) => {
                let mut resolved = real;
                for name in below.iter().rev() {
                    resolved.push(name);
                }
                return Ok(normalize(&resolved));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if std::fs::symlink_metadata(&current)
                    .is_ok_and(|found| found.file_type().is_symlink())
                {
                    // A link to nothing: what a write through it would create is where it points.
                    let target = std::fs::read_link(&current)?;
                    let beside = current.parent().map(Path::to_path_buf).unwrap_or_default();
                    current = normalize(&beside.join(target));
                } else if let Some(name) = current.file_name().map(OsString::from) {
                    below.push(name);
                    if !current.pop() {
                        return Err(error);
                    }
                } else {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::other("too many levels of symbolic links"))
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;
    use crate::scope::Mount;
    use crate::tools::ToolSet;

    /// A session in `directory` with the mounts given, in the mode given.
    fn scope(directory: &Path, mode: SandboxMode, mounts: Vec<Mount>) -> SessionScope {
        SessionScope {
            thread_id: "thread-1".to_owned(),
            tool_set: ToolSet::Plain,
            directory: directory.display().to_string(),
            cwd: None,
            sandbox_mode: mode,
            mounts,
        }
    }

    fn mount(name: &str, path: &Path, read_only: bool) -> Mount {
        Mount {
            name: name.to_owned(),
            path: path.display().to_string(),
            read_only,
        }
    }

    #[test]
    fn a_path_is_tidied_by_its_text() {
        assert_eq!(
            normalize(Path::new("/a/./b/../c//d")),
            PathBuf::from("/a/c/d")
        );
        assert_eq!(normalize(Path::new("/../..")), PathBuf::from("/"));
        assert_eq!(
            Access::absolute("x/../y", Path::new("/w")),
            PathBuf::from("/w/y")
        );
        assert_eq!(
            Access::absolute("/etc/passwd", Path::new("/w")),
            PathBuf::from("/etc/passwd")
        );
    }

    #[test]
    fn with_workspace_or_read_only_a_path_outside_the_directory_and_the_mounts_is_refused() {
        let outer = tempfile::tempdir().expect("a directory");
        let work = outer.path().join("work");
        let elsewhere = outer.path().join("elsewhere");
        std::fs::create_dir_all(&work).expect("work");
        std::fs::create_dir_all(&elsewhere).expect("elsewhere");
        for mode in [SandboxMode::Workspace, SandboxMode::ReadOnly] {
            let access = Access::new(&scope(&work, mode, vec![]), &[]);
            for path in [
                elsewhere.join("secret.txt"),
                work.join("../elsewhere/secret.txt"),
                PathBuf::from("/etc/passwd"),
            ] {
                let tidy = normalize(&path);
                let refusal = access
                    .permit(&tidy, Need::Read)
                    .expect_err("outside the directory");
                assert_eq!(refusal.code(), "outside_sandbox", "{path:?} in {mode:?}");
            }
            assert!(access.permit(&work.join("inside.txt"), Need::Read).is_ok());
        }
    }

    #[test]
    fn with_open_nothing_is_refused_by_the_path() {
        let outer = tempfile::tempdir().expect("a directory");
        let work = outer.path().join("work");
        std::fs::create_dir_all(&work).expect("work");
        let access = Access::new(&scope(&work, SandboxMode::Open, vec![]), &[]);
        let outside = outer.path().join("elsewhere/file");
        let permitted = access.permit(&outside, Need::Write).expect("open permits");
        assert!(permitted.ends_with("elsewhere/file"));
    }

    #[test]
    fn a_link_inside_the_directory_that_leads_out_is_refused_even_when_its_target_is_not_there_yet()
    {
        let outer = tempfile::tempdir().expect("a directory");
        let work = outer.path().join("work");
        let elsewhere = outer.path().join("elsewhere");
        std::fs::create_dir_all(&work).expect("work");
        std::fs::create_dir_all(&elsewhere).expect("elsewhere");
        symlink(&elsewhere, work.join("door")).expect("a link to a directory");
        symlink(elsewhere.join("later.txt"), work.join("dangling")).expect("a link to nothing");
        let access = Access::new(&scope(&work, SandboxMode::Workspace, vec![]), &[]);
        for through in ["door/new.txt", "door", "dangling"] {
            let path = work.join(through);
            let refusal = access
                .permit(&path, Need::Write)
                .expect_err("the link leads out");
            assert_eq!(refusal.code(), "outside_sandbox", "{through}");
        }
    }

    #[test]
    fn a_path_below_a_directory_that_does_not_exist_yet_is_judged_by_its_nearest_parent() {
        let outer = tempfile::tempdir().expect("a directory");
        let work = outer.path().join("work");
        std::fs::create_dir_all(&work).expect("work");
        let access = Access::new(&scope(&work, SandboxMode::Workspace, vec![]), &[]);
        let permitted = access
            .permit(&work.join("a/b/c.txt"), Need::Write)
            .expect("a new file below the directory");
        assert!(permitted.ends_with("work/a/b/c.txt"));
    }

    #[test]
    fn read_only_forbids_every_write_and_a_read_only_mount_forbids_the_writes_inside_it() {
        let outer = tempfile::tempdir().expect("a directory");
        let work = outer.path().join("work");
        let src = outer.path().join("src");
        let nested = work.join("vendor");
        for dir in [&work, &src, &nested] {
            std::fs::create_dir_all(dir).expect("directory");
        }
        let mounts = vec![
            mount("src", &src, true),
            mount("vendor", &nested, true),
            mount("scratch", &outer.path().join("scratch"), false),
        ];
        let access = Access::new(&scope(&work, SandboxMode::Workspace, mounts.clone()), &[]);
        assert!(access.permit(&work.join("a.txt"), Need::Write).is_ok());
        assert!(access.permit(&src.join("a.txt"), Need::Read).is_ok());
        let in_mount = access
            .permit(&src.join("a.txt"), Need::Write)
            .expect_err("read-only mount");
        assert!(matches!(in_mount, Refusal::ReadOnlyMount { .. }));
        let nested_write = access
            .permit(&nested.join("a.txt"), Need::Write)
            .expect_err("the mount inside the directory is read-only too");
        assert!(matches!(nested_write, Refusal::ReadOnlyMount { .. }));
        assert!(
            access
                .permit(&outer.path().join("scratch/x"), Need::Write)
                .is_ok(),
            "a mount that may be written"
        );
        let read_only = Access::new(&scope(&work, SandboxMode::ReadOnly, mounts), &[]);
        let refusal = read_only
            .permit(&work.join("a.txt"), Need::Write)
            .expect_err("read_only");
        assert!(matches!(refusal, Refusal::ReadOnlySession { .. }));
        assert!(read_only.permit(&work.join("a.txt"), Need::Read).is_ok());
    }

    #[test]
    fn no_command_is_run_when_the_mode_is_read_only() {
        let outer = tempfile::tempdir().expect("a directory");
        let read_only = Access::new(&scope(outer.path(), SandboxMode::ReadOnly, vec![]), &[]);
        assert_eq!(read_only.permit_commands(), Err(Refusal::NoCommands));
        let workspace = Access::new(&scope(outer.path(), SandboxMode::Workspace, vec![]), &[]);
        assert_eq!(workspace.permit_commands(), Ok(()));
    }

    #[test]
    fn the_kept_output_of_commands_may_be_read_and_never_written() {
        let outer = tempfile::tempdir().expect("a directory");
        let work = outer.path().join("work");
        let kept = outer.path().join("kept");
        std::fs::create_dir_all(&work).expect("work");
        std::fs::create_dir_all(&kept).expect("kept");
        let access = Access::new(
            &scope(&work, SandboxMode::Workspace, vec![]),
            std::slice::from_ref(&kept),
        );
        assert!(access.permit(&kept.join("out.log"), Need::Read).is_ok());
        assert!(access.permit(&kept.join("out.log"), Need::Write).is_err());
        assert!(
            !access.in_reach_of_the_thread(&kept),
            "it is no place for a command to work in"
        );
        assert!(access.in_reach_of_the_thread(&work));
    }
}
