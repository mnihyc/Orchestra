//! Attached mounts and their modes: reported state, live listing and reading, and further roots for executors.
//!
//! A mount is data that lives on the machine and belongs to no environment. For an environment
//! that a driver made, the driver attached them and wrote what it did into the file
//! `ORCH_MOUNTS_FILE` names; the host reads that file and reports it. For a directory on a
//! person's machine the host attaches them itself, from the locations the system names, and
//! checks each: the directory has to exist, it has to lie inside the roots the machine's owner
//! offers whatever the system says, and its place may not be, hold or lie inside the workspace,
//! the home or `/run/orch`, nor be the place of another mount.
//!
//! A mount stays where it is: nothing is copied. It is listed and read through Orch live, by
//! the same paths as the workspace but with the mount's directory as the root and no ignore rules,
//! and it is never written through Orch. Executors are given the attached mounts as further roots.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use orch_driver_contract::mounts::{
    AttachedMount, MountMode as DriverMode, TargetFault, check_targets,
};
use orch_link_contract::commands::{AttachMode, EnvAttachMount};
use orch_link_contract::events::{EnvironmentFactsMount, MountFault, MountMode, MountState};
use orch_types::Id;

use crate::environment::{GateState, Limits};
use crate::error::{HostError, Result};
use crate::files::Files;
use crate::index::HashCache;
use crate::paths::{PathStyle, Workspace};

/// The directory of the connector's run-time files, which no mount may cover or lie inside.
pub const RUN_DIR: &str = "/run/orch";

/// Whether the local kernel holds every read-only driver mount read-only.
/// A driver declaration alone is insufficient for this fact; an unreadable mount table claims nothing.
#[must_use]
pub fn driver_mounts_enforced(file: Option<&Path>) -> bool {
    let Some(file) = file else { return false };
    let Some(mounts) = std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<AttachedMount>>(&bytes).ok())
    else {
        return false;
    };
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/mountinfo")
            .ok()
            .is_some_and(|table| read_only_are_enforced(&mounts, &table))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = mounts;
        false
    }
}

#[cfg(any(target_os = "linux", test))]
fn read_only_are_enforced(mounts: &[AttachedMount], table: &str) -> bool {
    let read_only: Vec<_> = mounts
        .iter()
        .filter(|mount| mount.mode == DriverMode::ReadOnly)
        .collect();
    !read_only.is_empty()
        && read_only.iter().all(|mount| {
            let real_path = real(Path::new(&mount.path));
            table
                .lines()
                .filter_map(|line| {
                    let mut fields = line.split_whitespace();
                    let path = fields.nth(4)?;
                    let options = fields.next()?;
                    let path = path
                        .replace("\\040", " ")
                        .replace("\\011", "\t")
                        .replace("\\012", "\n")
                        .replace("\\134", "\\");
                    let path = PathBuf::from(path);
                    real_path
                        .starts_with(&path)
                        .then_some((path, options.split(',').any(|option| option == "ro")))
                })
                .max_by_key(|(path, _)| path.as_os_str().len())
                .is_some_and(|(_, read_only)| read_only)
        })
}

fn link_mode(mode: DriverMode) -> MountMode {
    match mode {
        DriverMode::ReadOnly => MountMode::ReadOnly,
        DriverMode::Shared => MountMode::Shared,
        DriverMode::Private => MountMode::Private,
    }
}

fn driver_mode(mode: MountMode) -> DriverMode {
    match mode {
        MountMode::ReadOnly => DriverMode::ReadOnly,
        MountMode::Shared => DriverMode::Shared,
        MountMode::Private => DriverMode::Private,
    }
}

fn asked_mode(mode: AttachMode) -> MountMode {
    match mode {
        AttachMode::ReadOnly => MountMode::ReadOnly,
        AttachMode::Shared => MountMode::Shared,
    }
}

#[derive(Debug)]
struct Entry {
    fact: EnvironmentFactsMount,
    name: String,
    files: Option<Files>,
}

/// The mounts of one environment.
#[derive(Debug, Default)]
pub struct Mounts {
    entries: Vec<Entry>,
}

/// What a person's machine lets be attached.
#[derive(Debug, Clone, Default)]
pub struct MountPolicy {
    /// The directories the machine's owner offers; `None` for a machine with no such limit.
    pub roots: Option<Vec<PathBuf>>,
}

/// The real path of `path`, or `path` itself where it cannot be resolved.
fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn inside(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(real(root)))
}

impl Mounts {
    /// No mounts.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// The mounts a driver attached, as the file `ORCH_MOUNTS_FILE` names lists them. `enforced`
    /// says whether the driver itself keeps writes out of a read-only mount.
    pub fn from_driver_file(
        path: &Path,
        enforced: bool,
        limits: &Limits,
        gates: &Arc<GateState>,
    ) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            HostError::io(format!("reading the list of mounts {}", path.display()), e)
        })?;
        let attached: Vec<AttachedMount> = serde_json::from_str(&text)
            .map_err(|e| HostError::Unsupported(format!("the list of mounts is not one: {e}")))?;
        let mut entries = Vec::new();
        for mount in attached {
            let mode = link_mode(mount.mode);
            let files = Self::open(Path::new(&mount.path), limits, gates);
            let fact = match &files {
                Some(_) => EnvironmentFactsMount {
                    mount_id: Id::nil(),
                    name: mount.name.clone(),
                    state: MountState::Attached,
                    reason: None,
                    path: Some(mount.path.clone()),
                    mode: Some(mode),
                    asked: mode,
                    enforced: enforced && mode == MountMode::ReadOnly,
                },
                None => Self::unavailable(Id::nil(), &mount.name, mode, MountFault::Missing),
            };
            entries.push(Entry {
                fact,
                name: mount.name,
                files,
            });
        }
        Ok(Self { entries })
    }

    /// Attaches `requests` in place on a person's machine, checking each against the machine's
    /// policy and against the places of the environment itself.
    #[must_use]
    pub fn attach_in_place(
        requests: &[EnvAttachMount],
        policy: &MountPolicy,
        reserved: &[&str],
        limits: &Limits,
        gates: &Arc<GateState>,
    ) -> Self {
        let targets: Vec<(&str, &str)> = requests
            .iter()
            .map(|r| (r.name.as_str(), r.source.as_str()))
            .collect();
        let left_out: BTreeMap<&str, TargetFault> =
            check_targets(&targets, reserved).into_iter().collect();
        let mut entries = Vec::new();
        for request in requests {
            let asked = asked_mode(request.mode);
            let source = PathBuf::from(&request.source);
            let fault = if left_out.contains_key(request.name.as_str()) {
                Some(MountFault::TargetConflict)
            } else if !source.is_dir() {
                Some(MountFault::Missing)
            } else if policy
                .roots
                .as_deref()
                .is_some_and(|roots| !inside(&real(&source), roots))
            {
                Some(MountFault::Refused)
            } else {
                None
            };
            let (fact, files) = match fault {
                Some(fault) => (
                    Self::unavailable(request.mount_id, &request.name, asked, fault),
                    None,
                ),
                None => {
                    let files = Self::open(&source, limits, gates);
                    match files {
                        Some(_) => (
                            EnvironmentFactsMount {
                                mount_id: request.mount_id,
                                name: request.name.clone(),
                                state: MountState::Attached,
                                reason: None,
                                path: Some(request.source.clone()),
                                mode: Some(asked),
                                asked,
                                // The data is used where it is: only the harness's sandbox and the permissions keep writes out.
                                enforced: false,
                            },
                            files,
                        ),
                        None => (
                            Self::unavailable(
                                request.mount_id,
                                &request.name,
                                asked,
                                MountFault::Missing,
                            ),
                            None,
                        ),
                    }
                }
            };
            entries.push(Entry {
                fact,
                name: request.name.clone(),
                files,
            });
        }
        Self { entries }
    }

    fn unavailable(
        mount_id: Id,
        name: &str,
        asked: MountMode,
        reason: MountFault,
    ) -> EnvironmentFactsMount {
        EnvironmentFactsMount {
            mount_id,
            name: name.to_owned(),
            state: MountState::Unavailable,
            reason: Some(reason),
            path: None,
            mode: None,
            asked,
            enforced: false,
        }
    }

    fn open(path: &Path, limits: &Limits, gates: &Arc<GateState>) -> Option<Files> {
        let workspace = Workspace::open(
            path,
            &[],
            PathStyle::Posix,
            crate::platform::DEFAULT_CASE_SENSITIVE,
        )
        .ok()?;
        Some(Files::mount(
            workspace,
            Arc::new(HashCache::new()),
            limits.clone(),
            Arc::clone(gates),
        ))
    }

    /// What became of each mount, for the environment's facts.
    #[must_use]
    pub fn facts(&self) -> Vec<EnvironmentFactsMount> {
        self.entries
            .iter()
            .map(|entry| entry.fact.clone())
            .collect()
    }

    /// The files of the attached mount called `name`.
    pub fn files(&self, name: &str) -> Result<&Files> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .and_then(|entry| entry.files.as_ref())
            .ok_or_else(|| HostError::NotFound(format!("there is no attached mount called {name}")))
    }

    /// The attached mounts as further roots for executors and harnesses: name, place and mode.
    #[must_use]
    pub fn roots(&self) -> Vec<AttachedMount> {
        self.entries
            .iter()
            .filter(|entry| entry.files.is_some())
            .filter_map(|entry| {
                Some(AttachedMount {
                    name: entry.name.clone(),
                    path: entry.fact.path.clone()?,
                    mode: driver_mode(entry.fact.mode?),
                })
            })
            .collect()
    }

    /// Writes the further roots in the form of the file `ORCH_MOUNTS_FILE` names.
    pub fn write_roots_file(&self, path: &Path) -> Result<()> {
        let text = orch_driver_contract::runtime::mounts_file(&self.roots());
        std::fs::write(path, text)
            .map_err(|e| HostError::io(format!("writing the list of mounts {}", path.display()), e))
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::environment::Gates;
    use crate::paths::RelPath;

    #[test]
    fn read_only_enforcement_is_reported_only_when_the_local_mount_table_holds_it() {
        let mounts = vec![AttachedMount {
            name: "data".into(),
            path: "/mnt/data".into(),
            mode: DriverMode::ReadOnly,
        }];
        let read_only = "1 0 0:1 / / rw - overlay overlay rw\n2 1 0:2 / /mnt/data ro,relatime - ext4 /dev/data rw\n";
        assert!(read_only_are_enforced(&mounts, read_only));
        assert!(!read_only_are_enforced(
            &mounts,
            &read_only.replace("ro,relatime", "rw,relatime")
        ));
        assert!(!read_only_are_enforced(&mounts, ""));
        assert!(!read_only_are_enforced(
            &mounts,
            "1 0 0:1 / / rw - overlay overlay rw\n"
        ));
    }

    fn gates() -> Arc<GateState> {
        Arc::new(GateState::new(Gates::OPEN))
    }

    fn request(name: &str, source: &Path, mode: AttachMode) -> EnvAttachMount {
        EnvAttachMount {
            volume: None,
            mount_id: orch_types::new_id(),
            name: name.to_owned(),
            source: source.to_string_lossy().into_owned(),
            mode,
        }
    }

    #[test]
    fn a_mount_is_attached_where_it_is_listed_and_read_live_and_never_written() {
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir(data.path().join("sub")).unwrap();
        std::fs::write(data.path().join("a.csv"), "1,2\n").unwrap();
        std::fs::write(data.path().join("debug.log"), "x").unwrap();
        let requests = [request("src", data.path(), AttachMode::ReadOnly)];
        let mounts = Mounts::attach_in_place(
            &requests,
            &MountPolicy::default(),
            &["/workspace"],
            &Limits::default(),
            &gates(),
        );
        let fact = &mounts.facts()[0];
        assert_eq!(fact.state, MountState::Attached);
        assert_eq!(fact.path.as_deref(), Some(data.path().to_str().unwrap()));
        assert_eq!(
            (fact.mode, fact.asked),
            (Some(MountMode::ReadOnly), MountMode::ReadOnly)
        );
        assert!(
            !fact.enforced,
            "used where it is, nothing but the sandbox and permissions keeps a write out"
        );

        let files = mounts.files("src").unwrap();
        let names: Vec<String> = files
            .list(&RelPath::root())
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(
            names,
            ["sub", "a.csv", "debug.log"],
            "no ignore rules apply to a mount"
        );
        assert!(matches!(
            files.make_dir(&RelPath::root().join("new").unwrap()),
            Err(HostError::Unsupported(_))
        ));
        assert!(mounts.files("other").is_err());
        let roots = mounts.roots();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].mode, DriverMode::ReadOnly);
    }

    #[test]
    fn a_mount_that_is_missing_refused_or_in_the_way_is_unavailable_with_the_reason() {
        let allowed = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("inside")).unwrap();
        let ws = workspace.path().to_str().unwrap().to_owned();
        let requests = [
            request("ok", allowed.path(), AttachMode::Shared),
            request(
                "missing",
                &allowed.path().join("gone"),
                AttachMode::ReadOnly,
            ),
            request("refused", elsewhere.path(), AttachMode::ReadOnly),
            request(
                "conflict",
                &workspace.path().join("inside"),
                AttachMode::ReadOnly,
            ),
            request("twin", allowed.path(), AttachMode::ReadOnly),
        ];
        let policy = MountPolicy {
            roots: Some(vec![
                allowed.path().to_path_buf(),
                workspace.path().to_path_buf(),
            ]),
        };
        let mounts = Mounts::attach_in_place(
            &requests,
            &policy,
            &[ws.as_str(), "/run/orch"],
            &Limits::default(),
            &gates(),
        );
        let by_name: BTreeMap<String, (MountState, Option<MountFault>)> = mounts
            .facts()
            .into_iter()
            .map(|f| (f.name, (f.state, f.reason)))
            .collect();
        assert_eq!(by_name["ok"], (MountState::Attached, None));
        assert_eq!(
            by_name["missing"],
            (MountState::Unavailable, Some(MountFault::Missing))
        );
        assert_eq!(
            by_name["refused"],
            (MountState::Unavailable, Some(MountFault::Refused)),
            "whatever the system says, the machine's owner decides"
        );
        assert_eq!(
            by_name["conflict"],
            (MountState::Unavailable, Some(MountFault::TargetConflict)),
            "inside the workspace"
        );
        assert_eq!(
            by_name["twin"],
            (MountState::Unavailable, Some(MountFault::TargetConflict)),
            "the place of another mount"
        );
        assert!(mounts.files("refused").is_err() && mounts.files("missing").is_err());
        assert_eq!(mounts.roots().len(), 1);
        assert_eq!(
            mounts
                .facts()
                .iter()
                .find(|f| f.name == "ok")
                .unwrap()
                .asked,
            MountMode::Shared
        );
    }

    #[test]
    fn the_mounts_a_driver_attached_are_read_from_its_file_and_reported_as_it_gave_them() {
        let data = tempfile::tempdir().unwrap();
        let file = tempfile::tempdir().unwrap();
        let list = file.path().join("mounts.json");
        let attached = [
            AttachedMount {
                name: "src".into(),
                path: data.path().to_string_lossy().into_owned(),
                mode: DriverMode::Private,
            },
            AttachedMount {
                name: "gone".into(),
                path: "/no/such/place/for/a/mount".into(),
                mode: DriverMode::ReadOnly,
            },
        ];
        std::fs::write(&list, orch_driver_contract::runtime::mounts_file(&attached)).unwrap();
        let mounts = Mounts::from_driver_file(&list, true, &Limits::default(), &gates()).unwrap();
        let facts = mounts.facts();
        assert_eq!(
            (facts[0].state, facts[0].mode),
            (MountState::Attached, Some(MountMode::Private))
        );
        assert_eq!(facts[1].reason, Some(MountFault::Missing));
        assert!(
            Mounts::from_driver_file(
                &file.path().join("nothing"),
                false,
                &Limits::default(),
                &gates()
            )
            .is_err()
        );

        let again = file.path().join("again.json");
        mounts.write_roots_file(&again).unwrap();
        let round: Vec<AttachedMount> =
            serde_json::from_str(&std::fs::read_to_string(again).unwrap()).unwrap();
        assert_eq!(round.len(), 1, "only what is attached is a root");
    }
}
