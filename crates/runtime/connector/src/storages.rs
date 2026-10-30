//! Named machine storages and pinned owned directories under the machine's allowed roots.

use orch_config::connector::ConnectorConfig;
use orch_driver_contract::runtime::DirectoryBinding;
use orch_link_contract::commands::{CommandError, ErrorCode};
use orch_link_contract::events::NodeStorage;
use orch_link_contract::policy::LocalPolicy;
use orch_types::Id;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
struct Storage {
    name: String,
    path: PathBuf,
    default: bool,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Kept {
    storage: String,
    directory: DirectoryBinding,
}

/// A directory made by the owned-data factory, with its existing proof retained for purge.
#[derive(Debug, Clone)]
pub struct OwnedData {
    record: PathBuf,
    /// The actual directory to show in the local uninstall plan.
    pub directory: DirectoryBinding,
}

/// Lists only existing ownership records; allowed roots and borrowed directories are never inferred.
pub fn owned_data(data_dir: &Path) -> Result<Vec<OwnedData>, CommandError> {
    let mut result = Vec::new();
    let records = match std::fs::read_dir(data_dir.join("storage-bindings")) {
        Ok(records) => records,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(_) => return Err(refused("the owned-data records cannot be read")),
    };
    for entry in records {
        let entry = entry.map_err(|_| refused("an owned-data record cannot be read"))?;
        if entry
            .path()
            .extension()
            .is_none_or(|extension| extension != "json")
        {
            continue;
        }
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            return Err(refused("an owned-data record is not a real file"));
        }
        let kept: Kept = serde_json::from_slice(
            &std::fs::read(entry.path())
                .map_err(|_| refused("an owned-data record cannot be read"))?,
        )
        .map_err(|_| refused("an owned-data record is invalid"))?;
        result.push(OwnedData {
            record: entry.path(),
            directory: kept.directory,
        });
    }
    result.sort_by(|left, right| left.record.cmp(&right.record));
    Ok(result)
}

/// Rechecks the pinned directory and its ownership record immediately before local uninstall removes it.
pub fn remove_owned(policy: &LocalPolicy, owned: &OwnedData) -> Result<(), CommandError> {
    let kept: Kept = serde_json::from_slice(
        &std::fs::read(&owned.record)
            .map_err(|_| refused("the owned-data record cannot be read"))?,
    )
    .map_err(|_| refused("the owned-data record is invalid"))?;
    if kept.directory != owned.directory {
        return Err(refused("the owned-data record changed after planning"));
    }
    let path = Path::new(&owned.directory.path);
    if std::fs::symlink_metadata(path).is_ok() {
        if crate::policy::inspect_environment_directory(policy, path)? != owned.directory {
            return Err(refused("the owned directory was replaced; it is kept"));
        }
        std::fs::remove_dir_all(path)
            .map_err(|_| refused("the owned directory could not be removed"))?;
    }
    std::fs::remove_file(&owned.record)
        .map_err(|_| refused("the owned-data record could not be cleared"))
}

fn refused(message: impl Into<String>) -> CommandError {
    CommandError::new(ErrorCode::PolicyRootNotAllowed, message)
}

fn declared(_config: &ConnectorConfig, policy: &LocalPolicy) -> Result<Vec<Storage>, CommandError> {
    let configured = if policy.storages.is_empty() {
        policy
            .environment_roots
            .iter()
            .enumerate()
            .map(|(index, path)| orch_config::server::NodeStorage {
                name: if index == 0 {
                    orch_driver_contract::volumes::DEFAULT_STORAGE.into()
                } else {
                    format!("root-{}", index + 1)
                },
                path: path.clone(),
                default: index == 0,
            })
            .collect()
    } else {
        policy.storages.clone()
    };
    let mut names = std::collections::BTreeSet::new();
    let mut defaults = 0;
    let mut storages = Vec::new();
    for storage in configured {
        if storage.name.trim().is_empty() || !names.insert(storage.name.clone()) {
            return Err(refused("machine storage names must be nonempty and unique"));
        }
        if storage.default {
            defaults += 1;
        }
        let directory =
            crate::policy::inspect_environment_directory(policy, Path::new(&storage.path))?;
        storages.push(Storage {
            name: storage.name,
            path: PathBuf::from(directory.path),
            default: storage.default,
        });
    }
    if !storages.is_empty() && defaults != 1 {
        return Err(refused(
            "the machine must declare exactly one default storage",
        ));
    }
    Ok(storages)
}

/// Reports the same storage shape as a node, without exposing local paths to the server.
#[must_use]
pub fn facts(config: &ConnectorConfig, policy: &LocalPolicy) -> Vec<NodeStorage> {
    declared(config, policy)
        .unwrap_or_default()
        .into_iter()
        .map(|storage| {
            #[cfg(unix)]
            let (capacity_gb, free_gb) = rustix::fs::statvfs(&storage.path)
                .map(|stat| {
                    // Storage reports everywhere use decimal gigabytes.
                    const BYTES_PER_GB: u64 = 1_000_000_000;
                    let gb = |blocks: u64| {
                        i64::try_from(blocks.saturating_mul(stat.f_frsize.max(1)) / BYTES_PER_GB)
                            .unwrap_or(i64::MAX)
                    };
                    (gb(stat.f_blocks), gb(stat.f_bavail))
                })
                .unwrap_or((0, 0));
            #[cfg(not(unix))]
            let (capacity_gb, free_gb) = (0, 0);
            NodeStorage {
                name: storage.name,
                default: storage.default,
                capacity_gb,
                free_gb,
                private_views: false,
            }
        })
        .collect()
}

/// Makes owned data on a selected storage, retaining its recorded location across later changes.
pub fn directory(
    data_dir: &Path,
    config: &ConnectorConfig,
    policy: &LocalPolicy,
    environment: Id,
    role: &str,
    requested: Option<&str>,
) -> Result<PathBuf, CommandError> {
    directory_generation(data_dir, config, policy, environment, role, requested, 0)
}

/// A move's fresh generation shares the same pinned storage/roots checks and never reuses old data.
pub fn directory_generation(
    data_dir: &Path,
    config: &ConnectorConfig,
    policy: &LocalPolicy,
    environment: Id,
    role: &str,
    requested: Option<&str>,
    generation: i64,
) -> Result<PathBuf, CommandError> {
    directory_generation_binding(
        data_dir,
        config,
        policy,
        environment,
        role,
        requested,
        generation,
    )
    .map(|(binding, _held)| PathBuf::from(binding.path))
}

/// Keeps the recorded identity and the directory it names together for attachment.
pub(crate) fn directory_generation_binding(
    data_dir: &Path,
    config: &ConnectorConfig,
    policy: &LocalPolicy,
    environment: Id,
    role: &str,
    requested: Option<&str>,
    generation: i64,
) -> Result<(DirectoryBinding, orch_envhost::paths::HeldRoot), CommandError> {
    if generation < 0 {
        return Err(refused("an owned-data generation is not negative"));
    }
    if !matches!(role, "workspace" | "home" | "state" | "mount") {
        return Err(refused(
            "the storage role is workspace, home, state or mount",
        ));
    }
    let records = data_dir.join("storage-bindings");
    let record = records.join(if generation == 0 {
        format!("{environment}-{role}.json")
    } else {
        format!("{environment}-g{generation}-{role}.json")
    });
    match std::fs::read(&record) {
        Ok(bytes) => {
            let kept: Kept = serde_json::from_slice(&bytes)
                .map_err(|_| refused("the kept data storage record cannot be read"))?;
            if requested.is_some_and(|name| name != kept.storage) {
                return Err(refused(
                    "existing data keeps its recorded storage; use an explicit move before choosing another",
                ));
            }
            let (actual, held) =
                crate::policy::hold_environment_directory(policy, Path::new(&kept.directory.path))?;
            if actual != kept.directory {
                return Err(refused(
                    "the kept data directory changed identity; it is not replaced",
                ));
            }
            return Ok((actual, held));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(refused("the kept data storage record cannot be opened")),
    }
    let storages = declared(config, policy)?;
    let storage = storages
        .iter()
        .find(|storage| requested.map_or(storage.default, |name| name == storage.name))
        .ok_or_else(|| {
            refused("the machine has no requested or default storage under its allowed roots")
        })?;
    let path = make(&storage.path, environment, role, generation)?;
    let (directory, held) = crate::policy::hold_environment_directory(policy, &path)?;
    #[cfg(target_os = "linux")]
    held.require_writable()
        .map_err(|error| refused(error.to_string()))?;
    #[cfg(not(target_os = "linux"))]
    crate::policy::writable_environment_directory(&path)?;
    let kept = Kept {
        storage: storage.name.clone(),
        directory,
    };
    std::fs::create_dir_all(&records)
        .map_err(|_| refused("the connector cannot create its storage records"))?;
    let bytes =
        serde_json::to_vec(&kept).map_err(|_| refused("the storage record cannot be encoded"))?;
    crate::policy::write_atomically(&record, &bytes)
        .map_err(|_| refused("the storage record cannot be saved"))?;
    Ok((kept.directory, held))
}

fn make(
    base: &Path,
    environment: Id,
    role: &str,
    generation: i64,
) -> Result<PathBuf, CommandError> {
    #[cfg(target_os = "linux")]
    {
        use rustix::fs::{Mode, OFlags, ResolveFlags, mkdirat, openat2};
        let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC;
        let resolve =
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS;
        let mut held = openat2(
            rustix::fs::CWD,
            base,
            flags,
            Mode::empty(),
            ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(|_| refused("the machine storage changed or cannot be opened"))?;
        let id = environment.to_string();
        let mut path = base.to_owned();
        // Each created component belongs to this connector; existing directories are never chmodded.
        let generation_name = format!("generation-{generation}");
        let mut components = vec!["orch-data", id.as_str()];
        if generation > 0 {
            components.push(generation_name.as_str());
        }
        components.push(role);
        for component in components {
            if let Err(error) = mkdirat(&held, component, Mode::RWXU) {
                if error != rustix::io::Errno::EXIST {
                    return Err(refused(
                        "owned data cannot be created on the selected storage",
                    ));
                }
                if component == role {
                    return Err(refused(
                        "an existing directory has no owned-data record; it is never adopted or purged",
                    ));
                }
            }
            held = openat2(&held, component, flags, Mode::empty(), resolve).map_err(|_| {
                refused("owned data must be a real directory beneath its selected storage")
            })?;
            path.push(component);
        }
        Ok(path)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (base, environment, role, generation);
        Err(CommandError::new(
            ErrorCode::Unsupported,
            "this machine cannot safely make owned directories beneath its storage roots",
        ))
    }
}

/// Idempotent cleanup of this factory's recorded data. A borrowed or replaced directory is never removed.
pub fn recorded_directory(
    data_dir: &Path,
    policy: &LocalPolicy,
    id: Id,
    role: &str,
) -> Result<Option<PathBuf>, CommandError> {
    if !matches!(role, "workspace" | "home" | "state" | "mount") {
        return Err(refused("unknown owned-data role"));
    }
    let record = data_dir
        .join("storage-bindings")
        .join(format!("{id}-{role}.json"));
    let kept: Kept = match std::fs::read(record) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| refused("the owned-data record cannot be read"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(refused("the owned-data record cannot be opened")),
    };
    let actual =
        crate::policy::inspect_environment_directory(policy, Path::new(&kept.directory.path))?;
    if actual != kept.directory {
        return Err(refused(
            "the owned data directory changed identity; it is not replaced",
        ));
    }
    Ok(Some(PathBuf::from(actual.path)))
}

/// Idempotent cleanup of this factory's recorded data. A borrowed or replaced directory is never removed.
pub fn remove_directory(
    data_dir: &Path,
    policy: &LocalPolicy,
    id: Id,
    role: &str,
) -> Result<(), CommandError> {
    remove_generation(data_dir, policy, id, role, 0)
}

/// Removes only a recorded generation; missing records mean no owned data, never a guessed path.
pub fn remove_generation(
    data_dir: &Path,
    policy: &LocalPolicy,
    id: Id,
    role: &str,
    generation: i64,
) -> Result<(), CommandError> {
    if generation < 0 {
        return Err(refused("an owned-data generation is not negative"));
    }
    if !matches!(role, "workspace" | "home" | "state" | "mount") {
        return Err(refused("unknown owned-data role"));
    }
    let record = data_dir.join("storage-bindings").join(if generation == 0 {
        format!("{id}-{role}.json")
    } else {
        format!("{id}-g{generation}-{role}.json")
    });
    let kept: Kept = match std::fs::read(&record) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| refused("the owned-data record cannot be read"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(refused("the owned-data record cannot be opened")),
    };
    let path = Path::new(&kept.directory.path);
    if path.exists() {
        if crate::policy::inspect_environment_directory(policy, path)? != kept.directory {
            return Err(refused("the data directory was replaced; it is not purged"));
        }
        std::fs::remove_dir_all(path)
            .map_err(|_| refused("the owned directory could not be removed"))?;
    }
    std::fs::remove_file(record)
        .map_err(|_| refused("the removed data record could not be cleared"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(target_os = "linux")]
    #[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
    fn an_owned_workspace_keeps_its_verified_handle_and_refuses_a_replacement() {
        use std::os::unix::fs::MetadataExt;

        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let policy = LocalPolicy {
            environment_roots: vec![root.path().to_string_lossy().into_owned()],
            ..Default::default()
        };
        let config = ConnectorConfig::default();
        let environment = orch_types::new_id();
        let (binding, held) = directory_generation_binding(
            data.path(),
            &config,
            &policy,
            environment,
            "workspace",
            None,
            0,
        )
        .unwrap();
        let original = PathBuf::from(&binding.path);
        let moved = original.with_extension("moved");
        let replacement = root.path().join("replacement");
        std::fs::create_dir(&replacement).unwrap();
        std::fs::rename(&original, &moved).unwrap();
        std::os::unix::fs::symlink(&replacement, &original).unwrap();

        let metadata = std::fs::metadata(&moved).unwrap();
        let still_held = held.metadata().unwrap();
        assert_eq!(
            (still_held.dev, still_held.ino),
            (metadata.dev(), metadata.ino())
        );
        let error = directory_generation_binding(
            data.path(),
            &config,
            &policy,
            environment,
            "workspace",
            None,
            0,
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::PolicyRootNotAllowed.as_str());
        assert!(error.detail.contains("changed identity"));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn a_move_generation_is_fresh_and_retry_keeps_it_without_replacing_an_earlier_directory() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let policy = LocalPolicy {
            environment_roots: vec![root.path().to_string_lossy().into_owned()],
            ..Default::default()
        };
        let config = ConnectorConfig::default();
        let environment = orch_types::new_id();
        let original = directory(
            state.path(),
            &config,
            &policy,
            environment,
            "workspace",
            None,
        )
        .unwrap();
        std::fs::write(original.join("keep"), "original").unwrap();
        let candidate = directory_generation(
            state.path(),
            &config,
            &policy,
            environment,
            "workspace",
            None,
            1,
        )
        .unwrap();
        assert_ne!(candidate, original);
        assert!(std::fs::read_dir(&candidate).unwrap().next().is_none());
        std::fs::write(candidate.join("copied"), "candidate").unwrap();
        assert_eq!(
            directory_generation(
                state.path(),
                &config,
                &policy,
                environment,
                "workspace",
                None,
                1
            )
            .unwrap(),
            candidate
        );
        let later = directory_generation(
            state.path(),
            &config,
            &policy,
            environment,
            "workspace",
            None,
            2,
        )
        .unwrap();
        assert_ne!(later, candidate);
        assert!(std::fs::read_dir(later).unwrap().next().is_none());
        assert_eq!(
            std::fs::read_to_string(original.join("keep")).unwrap(),
            "original"
        );
        assert_eq!(
            std::fs::read_to_string(candidate.join("copied")).unwrap(),
            "candidate"
        );
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn named_storages_choose_new_data_and_existing_data_never_moves_implicitly() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let config = ConnectorConfig {
            storage: vec![orch_config::server::NodeStorage {
                name: "fast".into(),
                path: root.path().to_string_lossy().into_owned(),
                default: true,
            }],
            ..Default::default()
        };
        let policy = LocalPolicy {
            environment_roots: vec![root.path().to_string_lossy().into_owned()],
            storages: config.storage.clone(),
            ..Default::default()
        };
        let id = orch_types::new_id();
        assert_eq!(facts(&config, &policy)[0].name, "fast");
        let made = directory(
            state.path(),
            &config,
            &policy,
            id,
            "workspace",
            Some("fast"),
        )
        .unwrap();
        assert!(made.starts_with(root.path()));
        std::fs::write(made.join("kept"), "work").unwrap();
        assert_eq!(
            directory(state.path(), &config, &policy, id, "workspace", None).unwrap(),
            made
        );
        assert!(
            directory(
                state.path(),
                &config,
                &policy,
                id,
                "workspace",
                Some("elsewhere")
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(made.join("kept")).unwrap(), "work");
        assert!(
            directory(
                state.path(),
                &config,
                &LocalPolicy::default(),
                id,
                "workspace",
                None
            )
            .is_err()
        );
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn directory_fact_uses_allowed_roots_and_never_falls_back_from_an_unknown_name() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let policy = LocalPolicy {
            environment_roots: vec![root.path().to_string_lossy().into_owned()],
            ..Default::default()
        };
        let config = ConnectorConfig::default();
        assert_eq!(facts(&config, &policy)[0].name, "default");
        assert!(
            directory(
                state.path(),
                &config,
                &policy,
                orch_types::new_id(),
                "home",
                Some("unknown")
            )
            .is_err()
        );
        assert!(facts(&config, &LocalPolicy::default()).is_empty());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn temporary_cleanup_removes_only_the_recorded_generation_and_is_safe_to_retry() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let policy = LocalPolicy {
            environment_roots: vec![root.path().to_string_lossy().into_owned()],
            ..Default::default()
        };
        let config = ConnectorConfig::default();
        let environment = orch_types::new_id();
        let original = directory(
            state.path(),
            &config,
            &policy,
            environment,
            "workspace",
            None,
        )
        .unwrap();
        let moved = directory_generation(
            state.path(),
            &config,
            &policy,
            environment,
            "workspace",
            None,
            1,
        )
        .unwrap();
        let borrowed = root.path().join("existing-project");
        std::fs::create_dir(&borrowed).unwrap();
        std::fs::write(original.join("keep"), "original").unwrap();
        std::fs::write(moved.join("remove"), "moved").unwrap();
        std::fs::write(borrowed.join("keep"), "borrowed").unwrap();
        remove_generation(state.path(), &policy, environment, "workspace", 1).unwrap();
        remove_generation(state.path(), &policy, environment, "workspace", 1).unwrap();
        // A borrowed home has no factory record and is never inferred from its path.
        remove_generation(state.path(), &policy, environment, "home", 1).unwrap();
        assert!(!moved.exists());
        assert_eq!(
            std::fs::read_to_string(original.join("keep")).unwrap(),
            "original"
        );
        assert_eq!(
            std::fs::read_to_string(borrowed.join("keep")).unwrap(),
            "borrowed"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn cleanup_refuses_a_replacement_directory_and_a_machine_that_no_longer_allows_its_root() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let policy = LocalPolicy {
            environment_roots: vec![root.path().to_string_lossy().into_owned()],
            ..Default::default()
        };
        let config = ConnectorConfig::default();
        let environment = orch_types::new_id();
        let made = directory(
            state.path(),
            &config,
            &policy,
            environment,
            "workspace",
            None,
        )
        .unwrap();
        assert!(
            remove_generation(
                state.path(),
                &LocalPolicy::default(),
                environment,
                "workspace",
                0
            )
            .is_err()
        );
        let earlier = made.with_extension("earlier");
        std::fs::rename(&made, &earlier).unwrap();
        std::fs::create_dir(&made).unwrap();
        std::fs::write(made.join("keep"), "replacement").unwrap();
        assert!(remove_generation(state.path(), &policy, environment, "workspace", 0).is_err());
        assert_eq!(
            std::fs::read_to_string(made.join("keep")).unwrap(),
            "replacement"
        );
        assert!(earlier.is_dir());
    }
}
