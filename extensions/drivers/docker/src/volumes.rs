//! Named persistent volumes on the host's storages.
//!
//! A unit's workspace, home and state are named volumes that outlive the unit. A host declares **storages**,
//! named directories on its disks; a volume asked to be on one is a Docker volume whose data directory is a
//! directory of that name below the storage's path (the daemon binds it), so the data lands on that disk and
//! nowhere else. A host that declares none has one storage, `default`, where the daemon keeps volumes anyway.
//!
//! The driver also owns the first step of making a unit usable: a volume made empty belongs to root, and a unit
//! runs as somebody else. [`set_roots`] gives the top directory of each volume to the unit's user before the unit
//! first starts, through a container that is made, filled and removed without ever running.

use std::collections::HashMap;
use std::path::PathBuf;

use bollard::models::{
    ContainerCreateBody, HostConfig, Mount, MountType, Volume, VolumeCreateRequest,
};
use bollard::query_parameters::{CreateContainerOptionsBuilder, RemoveContainerOptionsBuilder};
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::volumes::{
    DEFAULT_STORAGE, StorageFault, StorageInfo, VolumeRequest, resolve_storage,
};

use crate::archive::{Entry, tar_of};
use crate::client::{Daemon, DaemonFacts, map_error, status_of};
use crate::config::DockerConfig;
use crate::units::{LABEL_PREPARE, driver_labels, is_driver_label_set};

/// How many bytes a GB is in this system: 2^30, as a MB of memory is 2^20, and as the daemon's own `size=10G`.
pub const BYTES_PER_GB: u64 = 1 << 30;

/// Where the volumes of a helper container are mounted, one directory for each.
const HELPER_ROOT: &str = "/v";

/// The longest name a volume may have: the daemon's own limit.
const MAX_VOLUME_NAME: usize = 255;

/// Where a volume's data lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// Where the daemon keeps volumes.
    Daemon,
    /// A directory below a declared storage.
    Directory {
        /// The storage.
        storage: String,
        /// The directory as the node sees it.
        node_path: PathBuf,
        /// The directory as the daemon sees it.
        host_path: PathBuf,
    },
}

/// Whether `name` can be the name of a volume.
#[must_use]
pub fn is_volume_name(name: &str) -> bool {
    name.len() <= MAX_VOLUME_NAME
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// The storages of the host as the node reports them, without their sizes: declared ones, or the one the daemon
/// keeps volumes in.
fn named_storages(config: &DockerConfig) -> Vec<StorageInfo> {
    if config.storages.is_empty() {
        return orch_driver_contract::volumes::implicit_storages(0, 0);
    }
    config
        .storages
        .iter()
        .map(|storage| StorageInfo {
            name: storage.name.clone(),
            default: storage.default,
            capacity_gb: 0,
            free_gb: 0,
            private_views: false,
        })
        .collect()
}

/// Where a volume asked to be on `storage` goes.
pub fn place(
    config: &DockerConfig,
    storage: Option<&str>,
    name: &str,
) -> Result<Placement, DriverError> {
    let storages = named_storages(config);
    let chosen = resolve_storage(storage, &storages).map_err(|fault| match fault {
        StorageFault::Unknown(_) | StorageFault::NoDefault => {
            DriverError::InvalidSpec(fault.to_string())
        }
    })?;
    match config
        .storages
        .iter()
        .find(|declared| declared.name == chosen.name)
    {
        None => Ok(Placement::Daemon),
        Some(declared) => {
            let node_path = declared.path.join(name);
            Ok(Placement::Directory {
                storage: declared.name.clone(),
                host_path: config.host_path(&node_path),
                node_path,
            })
        }
    }
}

/// The request that makes the volume, and the options an existing one must have to be the same.
pub fn create_request(
    config: &DockerConfig,
    request: &VolumeRequest,
    placement: &Placement,
) -> VolumeCreateRequest {
    let mut labels: HashMap<String, String> = driver_labels(config).into_iter().collect();
    labels.extend(request.labels.clone());
    let driver_opts = match placement {
        Placement::Daemon => None,
        Placement::Directory { host_path, .. } => Some(HashMap::from([
            ("type".to_owned(), "none".to_owned()),
            ("o".to_owned(), "bind".to_owned()),
            (
                "device".to_owned(),
                host_path.to_string_lossy().into_owned(),
            ),
        ])),
    };
    VolumeCreateRequest {
        name: Some(request.name.clone()),
        driver: Some("local".to_owned()),
        driver_opts,
        labels: Some(labels),
        cluster_volume_spec: None,
    }
}

/// Whether the volume is one this driver made.
#[must_use]
pub fn is_driver_volume(volume: &Volume) -> bool {
    is_driver_label_set(&volume.labels)
}

/// Reads only a recorded volume on the requested storage, leaving every byte and label intact.
pub async fn identity(
    daemon: &Daemon,
    config: &DockerConfig,
    request: &VolumeRequest,
) -> Result<Option<orch_driver_contract::volumes::VolumeIdentity>, DriverError> {
    if !is_volume_name(&request.name) {
        return Err(DriverError::InvalidSpec("invalid volume name".into()));
    }
    let Some(actual) = daemon.volume(&request.name).await? else {
        return Ok(None);
    };
    let placement = place(config, request.storage.as_deref(), &request.name)?;
    let wanted = create_request(config, request, &placement);
    if !is_driver_volume(&actual) || actual.options != wanted.driver_opts.unwrap_or_default() {
        return Err(DriverError::InvalidSpec(
            "the existing volume has another driver or storage".into(),
        ));
    }
    Ok(Some(orch_driver_contract::volumes::VolumeIdentity {
        labels: actual.labels.into_iter().collect(),
    }))
}

/// Makes sure the volume exists on the storage it names; see `EnvironmentDriver::ensure_volume`.
pub async fn ensure(
    daemon: &Daemon,
    config: &DockerConfig,
    request: &VolumeRequest,
) -> Result<(), DriverError> {
    if !is_volume_name(&request.name) {
        return Err(DriverError::InvalidSpec(format!(
            "{:?} is not a volume name",
            request.name
        )));
    }
    let placement = place(config, request.storage.as_deref(), &request.name)?;
    let wanted = create_request(config, request, &placement);
    if let Some(existing) = daemon.volume(&request.name).await? {
        for key in [
            "orch.system",
            "orch.environment_id",
            "orch.provider_id",
            "orch.mount_id",
            "orch.recovery_job_id",
        ] {
            if let Some(value) = request.labels.get(key)
                && existing.labels.get(key) != Some(value)
            {
                return Err(DriverError::InvalidSpec(
                    "the existing volume belongs to another system or resource".into(),
                ));
            }
        }
        let wanted_options = wanted.driver_opts.clone().unwrap_or_default();
        if existing.options != wanted_options {
            return Err(DriverError::InvalidSpec(format!(
                "the volume {} exists on another storage than {}",
                request.name,
                request.storage.as_deref().unwrap_or(DEFAULT_STORAGE)
            )));
        }
        return Ok(());
    }
    if let Placement::Directory { node_path, .. } = &placement {
        let path = node_path.clone();
        tokio::task::spawn_blocking(move || std::fs::create_dir_all(&path))
            .await
            .map_err(|error| DriverError::Internal(error.to_string()))?
            .map_err(|error| {
                DriverError::Unavailable(format!(
                    "the directory of the volume {} could not be made: {error}",
                    request.name
                ))
            })?;
    }
    match daemon.docker().create_volume(wanted).await {
        Ok(_) => {
            tracing::info!(volume = %request.name, "made a volume");
            Ok(())
        }
        // Made at the same moment by another command.
        Err(error) if status_of(&error) == Some(409) => Ok(()),
        Err(error) => Err(map_error(error)),
    }
}

/// Removes a volume of the driver and its data; see `EnvironmentDriver::remove_volume`.
///
/// The data of a volume that lives in a directory of a storage is not removed by the daemon with the volume: it is
/// removed after, from the directory the volume says was its own. A volume that is already gone from the daemon
/// leaves the directory the storages keep for its name, which is removed as well, so that a removal that failed
/// half way (the files of a user the node may not remove) is finished by asking again.
pub async fn remove(daemon: &Daemon, config: &DockerConfig, name: &str) -> Result<(), DriverError> {
    remove_checked(daemon, config, name, None).await
}

/// Removes freshly inventoried data only while the native volume still proves its installation.
pub async fn remove_from_inventory(
    daemon: &Daemon,
    config: &DockerConfig,
    system: &str,
    name: &str,
) -> Result<(), DriverError> {
    remove_checked(daemon, config, name, Some(system)).await
}

async fn remove_checked(
    daemon: &Daemon,
    config: &DockerConfig,
    name: &str,
    system: Option<&str>,
) -> Result<(), DriverError> {
    // The name is joined to the paths of the storages below, so it must be nothing but a volume's name.
    if !is_volume_name(name) {
        return Err(DriverError::InvalidSpec(format!(
            "{name:?} is not a volume name"
        )));
    }
    let volume = daemon.volume(name).await?;
    if let Some(system) = system {
        let Some(found) = &volume else {
            return Ok(());
        };
        if found
            .labels
            .get(orch_driver_contract::runtime::LABEL_SYSTEM)
            .map(String::as_str)
            != Some(system)
        {
            return Err(DriverError::NotFound(
                "the volume no longer belongs to this installation".into(),
            ));
        }
    }
    if let Some(found) = &volume
        && !is_driver_volume(found)
    {
        return Err(DriverError::NotFound(format!(
            "the volume {name} is not one the driver made, and is left alone"
        )));
    }
    if volume.is_some() {
        match daemon
            .docker()
            .remove_volume(
                name,
                Some(bollard::query_parameters::RemoveVolumeOptionsBuilder::default().build()),
            )
            .await
        {
            Ok(()) => {}
            Err(error) if status_of(&error) == Some(404) => {}
            Err(error) if status_of(&error) == Some(409) => {
                return Err(DriverError::Internal(format!(
                    "the volume {name} is in use: {error}"
                )));
            }
            Err(error) => return Err(map_error(error)),
        }
    }
    let directories: Vec<PathBuf> = config
        .storages
        .iter()
        .map(|storage| storage.path.join(name))
        .filter(|node_path| match &volume {
            Some(found) => {
                found.options.get("device").map(String::as_str)
                    == Some(config.host_path(node_path).to_string_lossy().as_ref())
            }
            None => true,
        })
        .collect();
    if !directories.is_empty() {
        let name = name.to_owned();
        tokio::task::spawn_blocking(move || {
            for directory in directories {
                match std::fs::remove_dir_all(&directory) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        return Err(DriverError::Internal(format!(
                            "the data of the volume {name} could not be removed from {}: {error}; the node must be allowed to remove what the user of a unit wrote there",
                            directory.display()
                        )));
                    }
                    _ => {}
                }
            }
            Ok(())
        })
        .await
        .map_err(|error| DriverError::Internal(error.to_string()))??;
    }
    tracing::info!(volume = name, "removed a volume");
    Ok(())
}

/// Verifies a kept volume without creating a missing one or accepting somebody else's data.
pub async fn exists(
    daemon: &Daemon,
    config: &DockerConfig,
    request: &VolumeRequest,
) -> Result<bool, DriverError> {
    let placement = place(config, request.storage.as_deref(), &request.name)?;
    let Some(actual) = daemon.volume(&request.name).await? else {
        return Ok(false);
    };
    let wanted = create_request(config, request, &placement);
    let labels = &actual.labels;
    // The controller's volume command records the environment; an older unit-only
    // create records the system. Accept either recorded owner, reject a conflicting
    // owner when both are present, and always verify the driver's own labels.
    let owners = ["orch.environment_id", "orch.provider_id", "orch.system"];
    let owned = owners.iter().any(|key| {
        request
            .labels
            .get(*key)
            .is_some_and(|value| labels.get(*key) == Some(value))
    });
    let names_owner = owners.iter().any(|key| request.labels.contains_key(*key));
    if !is_driver_volume(&actual)
        || (names_owner && !owned)
        || wanted.labels.as_ref().is_some_and(|expected| {
            expected.iter().any(|(key, value)| {
                labels.get(key) != Some(value)
                    && !(owned && owners.contains(&key.as_str()) && !labels.contains_key(key))
            })
        })
    {
        return Err(DriverError::InvalidSpec(
            "the kept volume's ownership does not match its frozen reference".to_owned(),
        ));
    }
    if actual.options != wanted.driver_opts.unwrap_or_default() {
        return Err(DriverError::InvalidSpec(
            "the kept volume's storage does not match its frozen reference".to_owned(),
        ));
    }
    if let Placement::Directory { node_path, .. } = placement {
        match std::fs::metadata(node_path) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(DriverError::InvalidSpec(
                    "the kept storage is not a directory".to_owned(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(DriverError::Unavailable(error.to_string())),
        }
    }
    Ok(true)
}

/// The storages of the host with their sizes, as the node reports them.
pub fn storages(config: &DockerConfig, facts: &DaemonFacts) -> Vec<StorageInfo> {
    let measure = |path: &std::path::Path| -> (i64, i64) {
        match rustix::fs::statvfs(path) {
            Ok(stat) => {
                let unit = stat.f_frsize.max(1);
                let gb = |blocks: u64| {
                    i64::try_from(blocks.saturating_mul(unit) / BYTES_PER_GB).unwrap_or(i64::MAX)
                };
                (gb(stat.f_blocks), gb(stat.f_bavail))
            }
            Err(_) => (0, 0),
        }
    };
    let private_views = facts.private_views();
    if config.storages.is_empty() {
        // The daemon's own directory when the node sees it, otherwise the node's root: in a container that
        // is the disk the host keeps its containers on.
        let root = PathBuf::from(&facts.data_root);
        let at = if !facts.data_root.is_empty() && root.exists() {
            root
        } else {
            PathBuf::from("/")
        };
        let (capacity, free) = measure(&at);
        let mut implicit = orch_driver_contract::volumes::implicit_storages(capacity, free);
        for storage in &mut implicit {
            storage.private_views = private_views;
        }
        return implicit;
    }
    config
        .storages
        .iter()
        .map(|storage| {
            let (capacity_gb, free_gb) = measure(&storage.path);
            StorageInfo {
                name: storage.name.clone(),
                default: storage.default,
                capacity_gb,
                free_gb,
                private_views,
            }
        })
        .collect()
}

/// A volume whose top directory is to be given to a user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    /// The volume's name.
    pub volume: String,
    /// The directory's name inside the helper, which is also the name of the role.
    pub role: String,
    /// The directory's permissions.
    pub mode: u32,
    /// Directories to make inside the volume, as `(name, mode)`.
    pub directories: Vec<(String, u32)>,
}

/// Gives the top directory of each volume (and makes the directories asked for inside) to `ids`, with the
/// permissions given. The volumes are mounted into a container that is made from `image`, never started and
/// removed after; a tar put into it is extracted by the daemon with the user, the group and the mode of its entries, and
/// the tar entry that is the volume's top directory is reached from the directory above, which is why every
/// volume is mounted one level down. `helper` names the container; a leftover of that name from a run that was
/// cut off is removed first.
pub async fn set_roots(
    daemon: &Daemon,
    config: &DockerConfig,
    helper: &str,
    image: &str,
    roots: &[Root],
    ids: (u32, u32),
    mtime: u64,
) -> Result<(), DriverError> {
    let mut uncarried = Vec::new();
    for root in roots {
        let carried = daemon.volume(&root.volume).await?.is_some_and(|volume| {
            volume.labels.get("orch.carried").map(String::as_str) == Some("true")
        });
        if !carried {
            uncarried.push(root);
        }
    }
    // Imported roots already have their exact owners/modes, including an otherwise empty home.
    if uncarried.is_empty() {
        return Ok(());
    }
    let roots = &uncarried;
    remove_helper(daemon, helper).await?;
    let mounts: Vec<Mount> = roots
        .iter()
        .map(|root| Mount {
            typ: Some(MountType::VOLUME),
            source: Some(root.volume.clone()),
            target: Some(format!("{HELPER_ROOT}/{}", root.role)),
            ..Mount::default()
        })
        .collect();
    let mut labels: HashMap<String, String> = driver_labels(config).into_iter().collect();
    labels.insert(LABEL_PREPARE.to_owned(), helper.to_owned());
    let body = ContainerCreateBody {
        image: Some(image.to_owned()),
        cmd: Some(vec!["true".to_owned()]),
        labels: Some(labels),
        host_config: Some(HostConfig {
            mounts: Some(mounts),
            network_mode: Some("none".to_owned()),
            ..HostConfig::default()
        }),
        ..ContainerCreateBody::default()
    };
    daemon
        .docker()
        .create_container(
            Some(
                CreateContainerOptionsBuilder::default()
                    .name(helper)
                    .build(),
            ),
            body,
        )
        .await
        .map_err(map_error)?;
    let outcome = async {
        let mut entries = Vec::new();
        for root in roots {
            entries.push(Entry::Dir {
                path: root.role.clone(),
                mode: root.mode,
                uid: ids.0,
                gid: ids.1,
            });
            for (name, mode) in &root.directories {
                entries.push(Entry::Dir {
                    path: format!("{}/{name}", root.role),
                    mode: *mode,
                    uid: ids.0,
                    gid: ids.1,
                });
            }
        }
        daemon
            .put_archive(helper, HELPER_ROOT, tar_of(&entries, mtime)?)
            .await
    }
    .await;
    remove_helper(daemon, helper).await?;
    outcome
}

/// The passwd and group files of an image, read from a container that is made from it and never run: how a
/// user that a spec names only by its name gets its numbers.
pub async fn user_numbers(
    daemon: &Daemon,
    config: &DockerConfig,
    helper: &str,
    image: &str,
    name: &str,
) -> Result<(u32, u32), DriverError> {
    remove_helper(daemon, helper).await?;
    let mut labels: HashMap<String, String> = driver_labels(config).into_iter().collect();
    labels.insert(LABEL_PREPARE.to_owned(), helper.to_owned());
    let body = ContainerCreateBody {
        image: Some(image.to_owned()),
        cmd: Some(vec!["true".to_owned()]),
        labels: Some(labels),
        host_config: Some(HostConfig {
            network_mode: Some("none".to_owned()),
            ..HostConfig::default()
        }),
        ..ContainerCreateBody::default()
    };
    daemon
        .docker()
        .create_container(
            Some(
                CreateContainerOptionsBuilder::default()
                    .name(helper)
                    .build(),
            ),
            body,
        )
        .await
        .map_err(map_error)?;
    let outcome = async {
        let passwd = daemon.read_file(helper, "/etc/passwd").await?;
        passwd
            .as_deref()
            .and_then(|text| lookup_user(&String::from_utf8_lossy(text), name))
            .ok_or_else(|| {
                DriverError::InvalidSpec(format!("the user {name} does not exist in the image"))
            })
    }
    .await;
    remove_helper(daemon, helper).await?;
    outcome
}

/// The user and the primary group of `name` in the text of a passwd file.
#[must_use]
pub fn lookup_user(passwd: &str, name: &str) -> Option<(u32, u32)> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next()? != name {
            return None;
        }
        let _password = fields.next()?;
        let uid = fields.next()?.parse().ok()?;
        let gid = fields.next()?.parse().ok()?;
        Some((uid, gid))
    })
}

/// Removes a helper container, if there is one of that name that is the driver's.
pub async fn remove_helper(daemon: &Daemon, helper: &str) -> Result<(), DriverError> {
    let Some(found) = daemon.container(helper).await? else {
        return Ok(());
    };
    let ours = found
        .config
        .as_ref()
        .and_then(|config| config.labels.as_ref())
        .is_some_and(|labels| labels.get(LABEL_PREPARE).map(String::as_str) == Some(helper));
    if !ours {
        return Err(DriverError::Internal(format!(
            "a container named {helper} exists and is not a helper of the driver"
        )));
    }
    match daemon
        .docker()
        .remove_container(
            helper,
            Some(
                RemoveContainerOptionsBuilder::default()
                    .force(true)
                    .v(true)
                    .build(),
            ),
        )
        .await
    {
        Ok(()) => Ok(()),
        Err(error) if status_of(&error) == Some(404) => Ok(()),
        Err(error) => Err(map_error(error)),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config::{PathMap, StorageConfig};

    fn config() -> DockerConfig {
        DockerConfig {
            storages: vec![
                StorageConfig {
                    name: "fast".into(),
                    path: "/mnt/fast".into(),
                    default: true,
                },
                StorageConfig {
                    name: "bulk".into(),
                    path: "/mnt/bulk".into(),
                    default: false,
                },
            ],
            path_map: vec![PathMap {
                node: "/mnt".into(),
                host: "/srv/disks".into(),
            }],
            ..DockerConfig::default()
        }
    }

    #[test]
    fn a_volume_lands_in_a_directory_of_the_storage_it_names_or_of_the_default() {
        let config = config();
        let Placement::Directory {
            storage,
            node_path,
            host_path,
        } = place(&config, Some("bulk"), "orch-ws-1").unwrap()
        else {
            panic!("a directory");
        };
        assert_eq!(storage, "bulk");
        assert_eq!(node_path, PathBuf::from("/mnt/bulk/orch-ws-1"));
        assert_eq!(
            host_path,
            PathBuf::from("/srv/disks/bulk/orch-ws-1"),
            "the daemon's name for it"
        );
        let Placement::Directory { storage, .. } = place(&config, None, "x").unwrap() else {
            panic!("a directory");
        };
        assert_eq!(storage, "fast");
        assert!(matches!(
            place(&config, Some("slow"), "x"),
            Err(DriverError::InvalidSpec(_))
        ));
    }

    #[test]
    fn a_host_without_storages_keeps_volumes_where_the_daemon_does() {
        let config = DockerConfig::default();
        assert_eq!(place(&config, None, "x").unwrap(), Placement::Daemon);
        assert_eq!(
            place(&config, Some("default"), "x").unwrap(),
            Placement::Daemon
        );
        assert!(place(&config, Some("fast"), "x").is_err());
    }

    #[test]
    fn a_volume_on_a_storage_is_bound_to_its_directory_and_carries_the_labels() {
        let mut config = config();
        config.labels.insert("orch.test".into(), "t".into());
        let request = VolumeRequest {
            name: "orch-ws-1".into(),
            storage: Some("fast".into()),
            labels: BTreeMap::from([("orch.system".to_owned(), "prod".to_owned())]),
        };
        let placement = place(&config, request.storage.as_deref(), &request.name).unwrap();
        let made = create_request(&config, &request, &placement);
        let options = made.driver_opts.unwrap();
        assert_eq!(options["type"], "none");
        assert_eq!(options["o"], "bind");
        assert_eq!(options["device"], "/srv/disks/fast/orch-ws-1");
        let labels = made.labels.unwrap();
        assert_eq!(labels["orch.system"], "prod");
        assert_eq!(labels["orch.driver"], "docker");
        assert_eq!(labels["orch.test"], "t");
    }

    #[test]
    fn only_names_the_daemon_takes_are_volume_names() {
        assert!(is_volume_name(
            "orch-ws-0199abcd-1234-7000-8000-0000000000e1"
        ));
        assert!(
            !is_volume_name("../x")
                && !is_volume_name("")
                && !is_volume_name("-x")
                && !is_volume_name("a b")
        );
    }

    #[test]
    fn a_user_is_found_by_name_in_a_passwd_file() {
        let passwd = "root:x:0:0:root:/root:/bin/sh\nagent:x:1000:1001::/home/agent:/bin/sh\n";
        assert_eq!(lookup_user(passwd, "agent"), Some((1000, 1001)));
        assert_eq!(lookup_user(passwd, "root"), Some((0, 0)));
        assert_eq!(lookup_user(passwd, "nobody"), None);
        assert_eq!(lookup_user("garbage", "agent"), None);
    }
}
