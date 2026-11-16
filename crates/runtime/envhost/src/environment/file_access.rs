//! One native target selection for relative roots and intentional absolute paths.

use super::*;
use orch_link_contract::commands::FsAccess;
use orch_link_contract::policy::FileReach;

#[derive(Clone, Copy)]
pub(super) enum FileAct {
    View,
    Manage,
}

#[derive(Clone, Copy)]
pub(super) enum FileWalk {
    Through,
    Entry,
}

pub(super) struct FileTarget {
    pub files: Files,
    pub path: RelPath,
    pub shown: String,
}

impl FileTarget {
    pub fn shown_path(&self, path: &str) -> String {
        let base = self.path.as_str();
        if path == base {
            return self.shown.clone();
        }
        let suffix = if base.is_empty() {
            Some(path)
        } else {
            path.strip_prefix(base)
                .and_then(|tail| tail.strip_prefix('/'))
        };
        suffix.map_or_else(
            || path.to_owned(),
            |tail| {
                if self.shown.is_empty() {
                    tail.to_owned()
                } else {
                    format!("{}/{}", self.shown.trim_end_matches('/'), tail)
                }
            },
        )
    }

    pub fn stat(&self) -> Result<FileStat> {
        let mut stat = self.files.stat(&self.path)?;
        stat.entry.path = self.shown.clone();
        Ok(stat)
    }

    pub fn binding(
        &self,
        environment: &Environment,
        access: FsAccess,
        mount: Option<String>,
    ) -> crate::directory_pages::Binding {
        crate::directory_pages::Binding {
            environment: environment.id,
            attachment: environment.directory_attachment,
            generation: Some(environment.generation()),
            mount,
            path: format!(
                "{:?}:{:?}:{}:{}",
                access.view,
                access.manage,
                self.files.root().root_path().display(),
                self.shown
            ),
        }
    }
}

/// Existing ancestors are resolved on the environment's machine; creation retains a checked parent.
fn canonical_target(path: &Path, walk: FileWalk) -> Result<PathBuf> {
    let (mut ancestor, mut tail) = match walk {
        FileWalk::Through => (path.to_path_buf(), Vec::new()),
        FileWalk::Entry => match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => (parent.to_path_buf(), vec![name.to_os_string()]),
            _ => (path.to_path_buf(), Vec::new()),
        },
    };
    loop {
        match std::fs::canonicalize(&ancestor) {
            Ok(mut real) => {
                for name in tail.into_iter().rev() {
                    real.push(name);
                }
                return Ok(real);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = ancestor
                    .file_name()
                    .ok_or_else(|| HostError::io("resolving the native path", error))?
                    .to_os_string();
                tail.push(name);
                if !ancestor.pop() {
                    return Err(HostError::FileReachDenied);
                }
            }
            Err(error) => return Err(HostError::io("resolving the native path", error)),
        }
    }
}

impl Environment {
    pub(super) async fn target(
        self: &Arc<Self>,
        access: FsAccess,
        act: FileAct,
        mount: Option<&str>,
        path: &str,
        walk: FileWalk,
    ) -> Result<FileTarget> {
        let environment = Arc::clone(self);
        let mount = mount.map(str::to_owned);
        let path = path.to_owned();
        blocking(move || environment.file_target(access, act, mount.as_deref(), &path, walk)).await
    }

    /// Root-only producers still require an explicitly admitted act.
    pub(super) fn require_file_access(&self, access: FsAccess, act: FileAct) -> Result<()> {
        match act {
            FileAct::View => access.view,
            FileAct::Manage => access.manage,
        }
        .ok_or(HostError::FileReachDenied)?;
        Ok(())
    }

    /// The command names its bound; neither an absolute string nor a cached cursor grants it.
    pub(super) fn file_target(
        &self,
        access: FsAccess,
        act: FileAct,
        mount: Option<&str>,
        path: &str,
        walk: FileWalk,
    ) -> Result<FileTarget> {
        let requested = match act {
            FileAct::View => access.view,
            FileAct::Manage => access.manage,
        }
        .ok_or(HostError::FileReachDenied)?;
        let reach = requested.min(self.files.gates().get().files_reach);
        let base = self.files_of(mount)?;
        let native = Path::new(path);
        if !native.is_absolute() && reach == FileReach::Roots {
            return Ok(FileTarget {
                files: base.clone(),
                path: base.parse(path)?,
                shown: path.to_owned(),
            });
        }
        if native.is_absolute() && mount.is_some() {
            return Err(HostError::FileReachDenied);
        }
        let native = if native.is_absolute() {
            native.to_path_buf()
        } else {
            base.root().root_path().join(native)
        };
        let real = canonical_target(&native, walk)?;
        let mounts = self.mounts.roots();
        for files in std::iter::once(&self.files).chain(
            mounts
                .iter()
                .filter_map(|mount| self.mounts.files(&mount.name).ok()),
        ) {
            if let Some(relative) = files.root().relative_to_root(&real) {
                let relative = files.parse(&relative.to_string_lossy())?;
                return Ok(FileTarget {
                    files: files.clone(),
                    path: relative,
                    shown: path.to_owned(),
                });
            }
        }
        if reach != FileReach::Machine {
            return Err(HostError::FileReachDenied);
        }
        let root = real.ancestors().last().ok_or(HostError::FileReachDenied)?;
        let workspace = Workspace::open(
            root,
            &[],
            self.files.root().style(),
            self.files.root().case_sensitive(),
        )?;
        let relative = workspace
            .relative_to_root(&real)
            .ok_or(HostError::FileReachDenied)?;
        let files = Files::machine(
            workspace,
            Arc::new(HashCache::new()),
            self.files.limits().clone(),
            Arc::clone(self.files.gates()),
        );
        let relative = files.parse(&relative.to_string_lossy())?;
        Ok(FileTarget {
            files,
            path: relative,
            shown: path.to_owned(),
        })
    }
}
