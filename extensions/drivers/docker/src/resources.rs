//! Exact ownership-checked inventory and removal without daemon-wide pruning.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use bollard::query_parameters::{
    ListNetworksOptionsBuilder, ListVolumesOptionsBuilder, RemoveContainerOptionsBuilder,
    RemoveImageOptionsBuilder,
};
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::resources::{Resource, ResourceKind, ResourceRef};
use orch_driver_contract::runtime::LABEL_SYSTEM;

use crate::client::map_error;
use crate::driver::DockerDriver;
use crate::units::is_driver_label_set;

/// A published tool path has exactly the tool name and immutable version beneath the cache root.
const TOOL_PATH_COMPONENTS: usize = 2;

fn owned(labels: &HashMap<String, String>, system: &str) -> bool {
    !system.is_empty()
        && is_driver_label_set(labels)
        && labels
            .get(LABEL_SYSTEM)
            .is_some_and(|value| value == system)
}

fn image_kind(labels: &HashMap<String, String>) -> ResourceKind {
    if labels.contains_key("orch.kept.environment") {
        ResourceKind::KeptLayer
    } else if labels.contains_key(crate::build::LABEL_DEFINITION) {
        ResourceKind::Build
    } else {
        ResourceKind::Image
    }
}

fn reference(kind: ResourceKind, id: impl Into<String>) -> ResourceRef {
    ResourceRef {
        kind,
        id: id.into(),
    }
}

fn resource(
    kind: ResourceKind,
    id: String,
    name: String,
    size_bytes: Option<i64>,
    labels: HashMap<String, String>,
) -> Resource {
    Resource {
        resource: reference(kind, id),
        name,
        size_bytes: size_bytes.filter(|size| *size >= 0),
        labels: labels.into_iter().collect(),
        depends_on: Vec::new(),
        aliases: Vec::new(),
        last_used_at: None,
        busy: false,
    }
}

impl DockerDriver {
    /// Stops only the exact owned container selected in a confirmed local purge.
    pub async fn stop_inventoried(
        &self,
        system: &str,
        target: &ResourceRef,
        grace: std::time::Duration,
    ) -> Result<(), DriverError> {
        if target.kind != ResourceKind::Unit {
            return Err(DriverError::InvalidSpec(
                "only a unit can be stopped".into(),
            ));
        }
        if let Some(unit) = self
            .list_units(system)
            .await?
            .into_iter()
            .find(|unit| unit.reference == target.id)
        {
            return self.stop_unit(&unit, grace).await;
        }
        let container = self
            .daemon
            .container(&target.id)
            .await?
            .ok_or_else(|| DriverError::NotFound("the owned unit is absent".into()))?;
        let labels = container
            .config
            .as_ref()
            .and_then(|config| config.labels.as_ref())
            .cloned()
            .unwrap_or_default();
        if !owned(&labels, system) {
            return Err(DriverError::NotFound(
                "the unit no longer belongs to this installation".into(),
            ));
        }
        if container.state.as_ref().and_then(|state| state.paused) == Some(true) {
            self.daemon
                .docker()
                .unpause_container(&target.id)
                .await
                .map_err(map_error)?;
        }
        self.daemon
            .patient(grace + crate::units::STOP_REQUEST_SLACK)
            .stop_container(
                &target.id,
                Some(
                    bollard::query_parameters::StopContainerOptionsBuilder::default()
                        .t(i32::try_from(grace.as_secs()).unwrap_or(i32::MAX))
                        .build(),
                ),
            )
            .await
            .map_err(map_error)
    }

    /// Every object labelled for this installation, including stopped units and orphaned volumes.
    pub async fn inventory(&self, system: &str) -> Result<Vec<Resource>, DriverError> {
        if system.is_empty() {
            return Err(DriverError::InvalidSpec(
                "an installation label is required".into(),
            ));
        }
        let containers = self.daemon.inventory_containers().await?;
        let images = self.list_all_images().await?;
        let image_refs: BTreeMap<_, _> = images
            .iter()
            .filter(|image| {
                is_driver_label_set(&image.labels)
                    && image
                        .labels
                        .get(LABEL_SYSTEM)
                        .is_some_and(|system| !system.is_empty())
            })
            .map(|image| {
                (
                    image.id.clone(),
                    reference(image_kind(&image.labels), image.id.clone()),
                )
            })
            .collect();
        let mut foreign_uses = BTreeSet::new();
        for image in images.iter().filter(|image| !owned(&image.labels, system)) {
            if let Some(parent) = image_refs.get(&image.parent_id) {
                foreign_uses.insert(parent.clone());
            }
        }
        let mut found = Vec::new();
        for unit in containers {
            let labels = unit.labels.clone().unwrap_or_default();
            let ours = owned(&labels, system);
            let mut dependencies = Vec::new();
            if let Some(image) = unit.image_id.as_ref().and_then(|id| image_refs.get(id)) {
                dependencies.push(image.clone());
            }
            for mount in unit.mounts.iter().flatten() {
                if let Some(name) = &mount.name {
                    dependencies.push(reference(ResourceKind::Volume, name.clone()));
                }
                if let (Some(source), Some(cache)) = (&mount.source, &self.config.tool_cache.dir) {
                    let cache = self.config.host_path(cache);
                    if let Ok(path) = std::path::Path::new(source).strip_prefix(cache) {
                        let pieces: Vec<_> = path.components().collect();
                        if pieces.len() == TOOL_PATH_COMPONENTS {
                            dependencies.push(reference(
                                ResourceKind::ToolVersion,
                                path.to_string_lossy().into_owned(),
                            ));
                        }
                    }
                }
            }
            for network in unit
                .network_settings
                .as_ref()
                .and_then(|settings| settings.networks.as_ref())
                .into_iter()
                .flat_map(|networks| networks.values())
            {
                if let Some(id) = &network.network_id {
                    dependencies.push(reference(ResourceKind::Network, id.clone()));
                }
            }
            if ours {
                let Some(id) = unit.id else {
                    return Err(DriverError::Internal(
                        "an owned container has no identity".into(),
                    ));
                };
                let name = unit
                    .names
                    .as_ref()
                    .and_then(|names| names.first())
                    .map_or_else(|| id.clone(), |name| name.trim_start_matches('/').into());
                let mut item = resource(ResourceKind::Unit, id, name, unit.size_rw, labels);
                item.depends_on = dependencies;
                let state = unit.state.map(|state| state.to_string());
                item.busy = matches!(state.as_deref(), Some("running" | "paused" | "restarting"));
                found.push(item);
            } else {
                foreign_uses.extend(dependencies);
            }
        }
        let volumes = self
            .daemon
            .docker()
            .list_volumes(Some(ListVolumesOptionsBuilder::default().build()))
            .await
            .map_err(map_error)?;
        for volume in volumes
            .volumes
            .into_iter()
            .flatten()
            .filter(|volume| owned(&volume.labels, system))
        {
            found.push(resource(
                ResourceKind::Volume,
                volume.name.clone(),
                volume.name,
                volume.usage_data.map(|usage| usage.size),
                volume.labels,
            ));
        }
        for network in self
            .daemon
            .docker()
            .list_networks(Some(ListNetworksOptionsBuilder::default().build()))
            .await
            .map_err(map_error)?
        {
            let labels = network.labels.unwrap_or_default();
            if owned(&labels, system) {
                let Some(id) = network.id else {
                    return Err(DriverError::Internal(
                        "an owned network has no identity".into(),
                    ));
                };
                found.push(resource(
                    ResourceKind::Network,
                    id.clone(),
                    network.name.unwrap_or(id),
                    None,
                    labels,
                ));
            }
        }
        for image in images
            .into_iter()
            .filter(|image| owned(&image.labels, system))
        {
            let kind = image_kind(&image.labels);
            let name = image
                .repo_tags
                .first()
                .filter(|tag| tag.as_str() != "<none>:<none>")
                .cloned()
                .unwrap_or_else(|| image.id.clone());
            // Image sizes include layers shared with other images. Neither that virtual size nor
            // a kept archive's payload size proves physical bytes attributable to this object.
            let size = None;
            let aliases = image
                .repo_digests
                .iter()
                .filter_map(|reference| {
                    reference
                        .split_once('@')
                        .map(|(_, digest)| digest.to_owned())
                })
                .collect();
            let mut item = resource(kind, image.id, name, size, image.labels);
            item.aliases = aliases;
            if let Some(parent) = image_refs.get(&image.parent_id) {
                item.depends_on.push(parent.clone());
            }
            found.push(item);
        }
        for item in &mut found {
            item.busy |= foreign_uses.contains(&item.resource);
        }
        // A container outside this installation may have been given one of our cached tools. Its
        // identity stays private, but that dependency must still keep the owned tool from deletion.
        for used in foreign_uses
            .into_iter()
            .filter(|used| used.kind == ResourceKind::ToolVersion)
        {
            let mut item = resource(
                ResourceKind::ToolVersion,
                used.id.clone(),
                used.id,
                None,
                HashMap::from([(LABEL_SYSTEM.into(), system.into())]),
            );
            item.busy = true;
            found.push(item);
        }
        Ok(found)
    }

    /// Re-enumerates ownership and dependencies before removing the exact immutable identity.
    pub async fn remove_inventoried(
        &self,
        system: &str,
        target: &ResourceRef,
    ) -> Result<(), DriverError> {
        let _change = self.locks.lock(&format!("inventory:{}", target.id)).await;
        let _build = self
            .builds
            .try_acquire_many(self.config.build_parallel.max(1))
            .map_err(|_| DriverError::Busy("a build is using the inventory".into()))?;
        let inventory = self.inventory(system).await?;
        let selected = inventory
            .iter()
            .find(|item| &item.resource == target)
            .ok_or_else(|| DriverError::NotFound("the owned inventory object is absent".into()))?;
        if selected.busy
            || inventory
                .iter()
                .any(|item| item.depends_on.contains(target))
        {
            return Err(DriverError::Busy("the object is in use".into()));
        }
        match target.kind {
            ResourceKind::Unit => {
                // The plan selects networks and volumes independently. Lifecycle removal of an
                // environment may end its private network, but exact object cleanup cannot do so.
                self.daemon
                    .docker()
                    .remove_container(
                        &target.id,
                        Some(
                            RemoveContainerOptionsBuilder::default()
                                .force(false)
                                .v(false)
                                .build(),
                        ),
                    )
                    .await
                    .map_err(map_error)
            }
            ResourceKind::Volume => {
                crate::volumes::remove_from_inventory(
                    &self.daemon,
                    &self.config,
                    system,
                    &target.id,
                )
                .await
            }
            ResourceKind::Network => self
                .daemon
                .docker()
                .remove_network(&target.id)
                .await
                .map_err(map_error),
            ResourceKind::Image | ResourceKind::KeptLayer | ResourceKind::Build => {
                self.remove_inventory_image(system, target).await
            }
            ResourceKind::ToolVersion => Err(DriverError::Unsupported(
                "tool versions belong to the node cache".into(),
            )),
        }
    }

    async fn remove_inventory_image(
        &self,
        system: &str,
        target: &ResourceRef,
    ) -> Result<(), DriverError> {
        let image = self
            .daemon
            .image(&target.id)
            .await?
            .ok_or_else(|| DriverError::NotFound("the owned image is absent".into()))?;
        let labels = image
            .config
            .and_then(|config| config.labels)
            .unwrap_or_default();
        if !owned(&labels, system) || image_kind(&labels) != target.kind {
            return Err(DriverError::NotFound(
                "the image no longer has this ownership".into(),
            ));
        }
        let tags: Vec<_> = image
            .repo_tags
            .unwrap_or_default()
            .into_iter()
            .filter(|tag| tag != "<none>:<none>")
            .collect();
        let prefix = if target.kind == ResourceKind::KeptLayer {
            format!(
                "orch-keep-{}:",
                labels
                    .get("orch.kept.environment")
                    .ok_or_else(|| DriverError::InvalidSpec(
                        "the kept image has no owner".into()
                    ))?
            )
        } else {
            format!("{}:", self.config.image_repository)
        };
        if tags.iter().any(|tag| !tag.starts_with(&prefix)) {
            return Err(DriverError::Busy(
                "the image also has a name outside this installation's owned cache".into(),
            ));
        }
        // Docker refuses an id with several tags. Remove only its verified owned names, with native
        // in-use checks still enabled; never force a container's image away or prune its parents.
        for tag in tags {
            if self
                .daemon
                .image(&tag)
                .await?
                .and_then(|image| image.id)
                .as_deref()
                != Some(target.id.as_str())
            {
                return Err(DriverError::Busy(
                    "an owned image name changed during removal".into(),
                ));
            }
            self.remove_image_alone(&tag).await.map_err(map_error)?;
        }
        if self.daemon.image(&target.id).await?.is_some() {
            self.daemon
                .docker()
                .remove_image(
                    &target.id,
                    Some(
                        RemoveImageOptionsBuilder::default()
                            .force(false)
                            .noprune(true)
                            .build(),
                    ),
                    None,
                )
                .await
                .map_err(map_error)?;
        }
        Ok(())
    }
}
