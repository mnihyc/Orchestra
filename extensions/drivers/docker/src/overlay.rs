//! Private views: a mount's location laid under a layer that belongs to one environment.
//!
//! A private view lets an environment read a large tree and change it without anything of its changes reaching
//! the tree or any other environment: the mount's location is the lower layer of an overlay, and a directory in
//! the environment's own layer volume (`orch-over-<environment>-<mount>`) is the upper one. The daemon makes the
//! overlay when the unit starts, as a local volume with the options of an overlay mount, so that the unit sees it
//! as an ordinary volume at the mount's target. The layer lasts as long as the environment: stopping, starting and
//! making the unit again find the same changes.
//!
//! Making a view takes three steps that [`prepare`], [`crate::volumes::set_roots`] and [`create_views`] do in
//! order: find the directories the overlay is made of, make `upper` and `work` inside the layer, and make the
//! volume that mounts them.

use std::collections::{BTreeMap, HashMap};

use bollard::models::{Volume, VolumeCreateRequest};
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::mounts::{MountKind, MountLayer};
use orch_driver_contract::volumes::VolumeRequest;

use crate::client::{Daemon, map_error, status_of};
use crate::config::DockerConfig;
use crate::mounts::{View, is_overlay_safe, volume};
use crate::units::driver_labels;
use crate::volumes::{self, Root};

/// What follows the name of a layer to make the name of the volume the overlay is.
const VIEW_SUFFIX: &str = "-view";

/// The directory of a layer that holds the environment's own changes.
pub const UPPER: &str = "upper";

/// The directory of a layer the overlay works in; it must be on the same file system as `upper`.
pub const WORK: &str = "work";

/// The permissions of the two directories of a layer.
const LAYER_MODE: u32 = 0o755;

/// A view that is ready to be made: the directories it is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    /// The view.
    pub view: View,
    /// The directory of the layer volume, as the daemon names it.
    pub layer_directory: String,
    /// The location the view lies over, as the daemon names it.
    pub lower: String,
    /// The name of the volume that is the overlay.
    pub view_volume: String,
}

/// The name of the overlay volume that a layer has.
#[must_use]
pub fn view_name(layer: &MountLayer) -> String {
    format!("{}{VIEW_SUFFIX}", layer.name)
}

/// The directory where the daemon keeps the data of a volume: the directory it is bound to when it is bound to
/// one, otherwise its own.
#[must_use]
pub fn data_directory(volume: &Volume) -> String {
    match (volume.options.get("o"), volume.options.get("device")) {
        (Some(options), Some(device)) if options.split(',').any(|option| option == "bind") => {
            device.clone()
        }
        _ => volume.mountpoint.clone(),
    }
}

/// The options of the volume that is the overlay.
#[must_use]
pub fn view_options(lower: &str, layer_directory: &str) -> HashMap<String, String> {
    HashMap::from([
        ("type".to_owned(), "overlay".to_owned()),
        ("device".to_owned(), "overlay".to_owned()),
        (
            "o".to_owned(),
            format!(
                "lowerdir={lower},upperdir={layer_directory}/{UPPER},workdir={layer_directory}/{WORK}"
            ),
        ),
    ])
}

/// Finds the directories each view is made of, and makes sure the layer volumes are there. A layer made here records
/// `owner` as the unit's own volumes do, so that a kept layer is proven to be this system's when it is restored.
pub async fn prepare(
    daemon: &Daemon,
    config: &DockerConfig,
    views: &[View],
    owner: &BTreeMap<String, String>,
) -> Result<Vec<Prepared>, DriverError> {
    let mut prepared = Vec::with_capacity(views.len());
    for view in views {
        volumes::ensure(
            daemon,
            config,
            &VolumeRequest {
                name: view.layer.name.clone(),
                storage: view.layer.storage.clone(),
                labels: owner.clone(),
            },
        )
        .await?;
        let layer = daemon.volume(&view.layer.name).await?.ok_or_else(|| {
            DriverError::Internal(format!("the layer {} is gone", view.layer.name))
        })?;
        let layer_directory = data_directory(&layer);
        let lower = match view.mount.kind {
            MountKind::Path => view.mount.source.clone(),
            MountKind::Volume => {
                let location = daemon.volume(&view.mount.source).await?.ok_or_else(|| {
                    DriverError::NotFound(format!(
                        "the volume {} of the mount {} does not exist",
                        view.mount.source, view.mount.name
                    ))
                })?;
                data_directory(&location)
            }
        };
        if !is_overlay_safe(&lower) || !is_overlay_safe(&layer_directory) {
            return Err(DriverError::InvalidSpec(format!(
                "the mount {} lies where an overlay cannot be made",
                view.mount.name
            )));
        }
        prepared.push(Prepared {
            view: view.clone(),
            layer_directory,
            lower,
            view_volume: view_name(&view.layer),
        });
    }
    Ok(prepared)
}

/// The directories to be made in the layers, as roots for the helper that makes them: `upper` and `work`, owned
/// by whoever the helper is told.
#[must_use]
pub fn layer_roots(prepared: &[Prepared]) -> Vec<Root> {
    prepared
        .iter()
        .enumerate()
        .map(|(index, one)| Root {
            volume: one.view.layer.name.clone(),
            role: format!("layer{index}"),
            mode: LAYER_MODE,
            directories: vec![
                (UPPER.to_owned(), LAYER_MODE),
                (WORK.to_owned(), LAYER_MODE),
            ],
        })
        .collect()
}

/// Makes the volumes that are the overlays, and answers the mounts that attach them and the names of the
/// volumes that this call made (a caller that fails later removes those). An overlay volume that exists over
/// other directories than these is made again.
pub async fn create_views(
    daemon: &Daemon,
    config: &DockerConfig,
    prepared: &[Prepared],
) -> Result<(Vec<bollard::models::Mount>, Vec<String>), DriverError> {
    let mut mounts = Vec::with_capacity(prepared.len());
    let mut made = Vec::new();
    for one in prepared {
        let wanted = view_options(&one.lower, &one.layer_directory);
        match daemon.volume(&one.view_volume).await? {
            Some(existing) if existing.options == wanted => {}
            Some(_) => {
                daemon
                    .docker()
                    .remove_volume(
                        &one.view_volume,
                        None::<bollard::query_parameters::RemoveVolumeOptions>,
                    )
                    .await
                    .map_err(map_error)?;
                create(daemon, config, one, wanted).await?;
                made.push(one.view_volume.clone());
            }
            None => {
                create(daemon, config, one, wanted).await?;
                made.push(one.view_volume.clone());
            }
        }
        mounts.push(volume(&one.view_volume, &one.view.mount.target, false));
    }
    Ok((mounts, made))
}

async fn create(
    daemon: &Daemon,
    config: &DockerConfig,
    one: &Prepared,
    options: HashMap<String, String>,
) -> Result<(), DriverError> {
    let labels: HashMap<String, String> = driver_labels(config).into_iter().collect();
    let request = VolumeCreateRequest {
        name: Some(one.view_volume.clone()),
        driver: Some("local".to_owned()),
        driver_opts: Some(options),
        labels: Some(labels),
        cluster_volume_spec: None,
    };
    match daemon.docker().create_volume(request).await {
        Ok(_) => Ok(()),
        Err(error) if status_of(&error) == Some(409) => Ok(()),
        Err(error) => Err(map_error(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume_with(options: &[(&str, &str)], mountpoint: &str) -> Volume {
        Volume {
            name: "v".into(),
            driver: "local".into(),
            mountpoint: mountpoint.into(),
            options: options
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            ..Volume::default()
        }
    }

    #[test]
    fn the_overlay_lies_over_the_location_with_its_changes_in_the_layer() {
        let options = view_options("/srv/src", "/var/lib/docker/volumes/orch-over-1/_data");
        assert_eq!(options["type"], "overlay");
        assert_eq!(options["device"], "overlay");
        assert_eq!(
            options["o"],
            "lowerdir=/srv/src,upperdir=/var/lib/docker/volumes/orch-over-1/_data/upper,workdir=/var/lib/docker/volumes/orch-over-1/_data/work"
        );
    }

    #[test]
    fn the_data_of_a_volume_is_where_the_daemon_keeps_it_or_where_it_is_bound() {
        assert_eq!(
            data_directory(&volume_with(&[], "/var/lib/docker/volumes/v/_data")),
            "/var/lib/docker/volumes/v/_data"
        );
        assert_eq!(
            data_directory(&volume_with(
                &[("type", "none"), ("o", "bind"), ("device", "/mnt/fast/v")],
                "/var/lib/docker/volumes/v/_data"
            )),
            "/mnt/fast/v"
        );
    }

    #[test]
    fn the_overlay_volume_of_a_layer_is_named_after_it() {
        let layer = MountLayer {
            name: "orch-over-e1-m1".into(),
            storage: None,
        };
        assert_eq!(view_name(&layer), "orch-over-e1-m1-view");
    }
}
