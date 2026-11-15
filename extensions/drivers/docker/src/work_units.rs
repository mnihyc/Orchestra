//! Disposable program scratch beside a provider, with no connector identity or personal storage.
use std::collections::{BTreeMap, BTreeSet, HashMap};

use bollard::models::{ContainerCreateBody, HostConfig};
use bollard::query_parameters::{CreateContainerOptionsBuilder, StartContainerOptions};
use orch_driver_contract::{
    driver::DriverError,
    runtime::{LABEL_KIND, LABEL_RUNTIME_ID, LABEL_SYSTEM, RuntimeKind},
    sources::Source,
    units::UnitRef,
    work::WorkUnitSpec,
};
use orch_types::Hash;
use serde_json::json;

use crate::{
    DockerDriver,
    archive::{Entry, tar_of},
    client::map_error,
    restricted,
};

pub(crate) const LABEL_WORK: &str = "orch.capability_work";
const LABEL_INPUT: &str = "orch.capability_input";
const LABEL_PROVIDER: &str = "orch.capability_provider_runtime";
const LABEL_DEADLINE: &str = "orch.capability_deadline";
// Linux's unprivileged nobody identity; each unit has its own filesystem and PID namespace.
const WORK_UID: u32 = 65_534;
const SCRATCH: &str = "/work";
const PREPARED: &str = "work-prepared";

fn input_archive(spec: &WorkUnitSpec, at: u64) -> Result<Vec<u8>, DriverError> {
    let mut files = BTreeSet::new();
    for file in &spec.files {
        if file.path.contains(['\\', '\0'])
            || file
                .path
                .split('/')
                .any(|part| part.is_empty() || matches!(part, "." | ".."))
            || !files.insert(file.path.as_str())
        {
            return Err(DriverError::InvalidSpec(
                "frozen input files need distinct relative scratch paths".into(),
            ));
        }
    }
    let mut directories =
        BTreeSet::from(["home".to_owned(), "inputs".to_owned(), "tmp".to_owned()]);
    for file in &spec.files {
        let mut parent = std::path::Path::new(&file.path).parent();
        while let Some(path) = parent {
            if path.as_os_str().is_empty() {
                break;
            }
            let path = path
                .to_str()
                .ok_or_else(|| DriverError::InvalidSpec("the scratch path is unreadable".into()))?;
            if files.contains(path) {
                return Err(DriverError::InvalidSpec(
                    "a frozen input cannot be a file and a directory".into(),
                ));
            }
            directories.insert(path.to_owned());
            parent = std::path::Path::new(path).parent();
        }
    }
    if directories
        .iter()
        .any(|directory| files.contains(directory.as_str()))
    {
        return Err(DriverError::InvalidSpec(
            "a scratch directory cannot also be an input file".into(),
        ));
    }
    let mut entries: Vec<Entry> = directories
        .into_iter()
        .map(|path| Entry::Dir {
            path,
            mode: 0o700,
            uid: WORK_UID,
            gid: WORK_UID,
        })
        .collect();
    entries.extend(spec.files.iter().map(|file| Entry::File {
        path: file.path.clone(),
        content: file.bytes.clone(),
        mode: 0o600,
        uid: WORK_UID,
        gid: WORK_UID,
    }));
    tar_of(&entries, at)
}

fn limits_of(parent: &HostConfig) -> HostConfig {
    HostConfig {
        nano_cpus: parent.nano_cpus,
        cpu_shares: parent.cpu_shares,
        cpu_quota: parent.cpu_quota,
        cpu_period: parent.cpu_period,
        cpuset_cpus: parent.cpuset_cpus.clone(),
        cpuset_mems: parent.cpuset_mems.clone(),
        memory: parent.memory,
        memory_swap: parent.memory_swap,
        memory_reservation: parent.memory_reservation,
        memory_swappiness: parent.memory_swappiness,
        pids_limit: parent.pids_limit,
        blkio_weight: parent.blkio_weight,
        storage_opt: parent.storage_opt.clone(),
        runtime: parent.runtime.clone(),
        init: Some(true),
        readonly_rootfs: Some(true),
        cap_drop: Some(vec!["ALL".into()]),
        privileged: Some(false),
        security_opt: Some(vec!["no-new-privileges".into()]),
        tmpfs: Some(HashMap::from([(
            SCRATCH.into(),
            format!(
                "rw,nosuid,nodev,mode=0700,uid={WORK_UID},gid={WORK_UID}{}",
                parent
                    .storage_opt
                    .as_ref()
                    .and_then(|options| options.get("size"))
                    .map_or_else(String::new, |size| format!(",size={size}"))
            ),
        )])),
        ..HostConfig::default()
    }
}

impl DockerDriver {
    pub(crate) async fn create_work_unit(
        &self,
        spec: &WorkUnitSpec,
    ) -> Result<UnitRef, DriverError> {
        // Validate material before making any host resource. No directory of the source is walked.
        let archive = input_archive(spec, self.clock.now().timestamp().try_into().unwrap_or(0))?;
        let seconds = spec
            .deadline
            .get()
            .signed_duration_since(self.clock.now())
            .num_seconds();
        if seconds <= 0 {
            return Err(DriverError::InvalidSpec(
                "the capability work's lifetime has ended".into(),
            ));
        }
        let parent = self
            .container_of(&spec.provider)
            .await?
            .ok_or_else(|| DriverError::NotFound("the provider unit is absent".into()))?;
        let parent_config = parent.config.as_ref().ok_or_else(|| {
            DriverError::Unavailable("the provider unit has no configuration".into())
        })?;
        let parent_labels = parent_config.labels.as_ref().ok_or_else(|| {
            DriverError::Unavailable("the provider unit has no ownership labels".into())
        })?;
        if parent_labels.contains_key(LABEL_WORK) {
            return Err(DriverError::InvalidSpec(
                "a capability unit cannot create another unit".into(),
            ));
        }
        let system = parent_labels.get(LABEL_SYSTEM).ok_or_else(|| {
            DriverError::InvalidSpec("the provider unit has no system identity".into())
        })?;
        let parent_host = parent.host_config.as_ref().ok_or_else(|| {
            DriverError::Unavailable("the provider unit has no resource boundary".into())
        })?;
        let runtime = parent_host
            .runtime
            .as_deref()
            .unwrap_or(&self.facts().default_runtime);
        if crate::runtimes::boundary_of(&self.config, runtime).has_kernel_of_its_own()
            && !self
                .restricted_proved
                .get(runtime)
                .copied()
                .unwrap_or(false)
        {
            return Err(DriverError::Unsupported(
                "network.restricted_unavailable: this runtime has no proved restricted network"
                    .into(),
            ));
        }
        let name = format!("orch-work-{}", spec.work.simple());
        let _held = self.locks.lock(&name).await;
        let files: Vec<_> = spec
            .files
            .iter()
            .map(|file| json!({"path":file.path,"hash":Hash::of(&file.bytes)}))
            .collect();
        let input = Hash::of(&serde_json::to_vec(&json!({"work":spec.work,"provider":spec.provider,
            "template":spec.template,"tools":spec.tools,"endpoints":spec.endpoints,"deadline":spec.deadline,"files":files}))
            .map_err(|_| DriverError::InvalidSpec("the frozen work identity is unreadable".into()))?).to_string();
        if let Some(existing) = self.daemon.container(&name).await? {
            let labels = existing
                .config
                .as_ref()
                .and_then(|config| config.labels.as_ref());
            if labels.is_none_or(|labels| {
                labels.get(LABEL_INPUT) != Some(&input)
                    || labels.get(LABEL_SYSTEM) != Some(system)
                    || labels.get(LABEL_WORK) != Some(&spec.work.to_string())
                    || labels.get(LABEL_PROVIDER) != Some(&spec.provider.runtime_id.to_string())
            }) {
                return Err(DriverError::InvalidSpec(
                    "this work identity already belongs to different frozen material".into(),
                ));
            }
            let dir = restricted::unit_dir(&self.config, &name)?;
            let prepared: Option<String> = crate::egress::files::read_json(&dir, PREPARED).await;
            if prepared.as_ref().is_some_and(|kept| kept != &input) {
                return Err(DriverError::InvalidSpec(
                    "the prepared work belongs to different frozen material".into(),
                ));
            }
            let unit = UnitRef {
                reference: existing.id.ok_or_else(|| {
                    DriverError::Unavailable("the work unit has no immutable identity".into())
                })?,
                name: name.clone(),
                runtime_id: spec.work,
                kind: RuntimeKind::Provider,
            };
            if prepared.is_some() {
                if existing.state.as_ref().and_then(|state| state.running) != Some(true) {
                    return Err(DriverError::Unavailable(
                        "this work unit has ended; it was not restarted with a fresh scratch home"
                            .into(),
                    ));
                }
                return Ok(unit);
            }
            // No program receives this unit until preparation is durable. An interrupted bootstrap
            // may be repeated, while a returned unit is never silently restarted.
            if existing.state.as_ref().and_then(|state| state.running) == Some(true) {
                self.daemon
                    .put_archive(&unit.reference, SCRATCH, archive)
                    .await?;
                crate::egress::files::write_json(&dir, PREPARED, &input)
                    .await
                    .map_err(|_| {
                        DriverError::Unavailable("the prepared work could not be retained".into())
                    })?;
                return Ok(unit);
            }
            self.daemon
                .docker()
                .remove_container(
                    &unit.reference,
                    Some(
                        bollard::query_parameters::RemoveContainerOptionsBuilder::default()
                            .force(true)
                            .build(),
                    ),
                )
                .await
                .map_err(map_error)?;
        }
        // Only the original template source is used: kept container files, mounts, user and env are excluded.
        let source = Source::from_spec(&spec.template).map_err(|_| {
            DriverError::InvalidSpec("the provider's base source is unreadable".into())
        })?;
        let image = self.image_for_source(&source).await?;
        let image = self
            .daemon
            .docker()
            .inspect_image(&image)
            .await
            .map_err(map_error)?;
        if image
            .config
            .as_ref()
            .and_then(|config| config.volumes.as_ref())
            .is_some_and(|volumes| !volumes.is_empty())
        {
            return Err(DriverError::Unsupported("this base image declares persistent volumes; the placement cannot give this program a disposable work unit".into()));
        }
        let image_id = image.id.ok_or_else(|| {
            DriverError::Unavailable("the program image has no immutable identity".into())
        })?;
        let mut env: BTreeMap<String, String> = image
            .config
            .and_then(|config| config.env)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|entry| {
                entry
                    .split_once('=')
                    .map(|(name, _)| (name.to_owned(), String::new()))
            })
            .collect();
        env.insert("HOME".into(), "/work/home".into());
        env.insert("TMPDIR".into(), "/work/tmp".into());
        env.insert(
            "PATH".into(),
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
        );
        let mut host = limits_of(parent_host);
        // Match actual verified cache sources, rather than trusting the destination of a personal mount.
        let mut mounts = Vec::new();
        for tool in &spec.tools {
            if [&tool.name, &tool.version].into_iter().any(|part| {
                part.is_empty()
                    || part.contains(['/', '\\', '\0'])
                    || matches!(part.as_str(), "." | "..")
            }) {
                return Err(DriverError::InvalidSpec(
                    "an installed program build needs a single name and version".into(),
                ));
            }
            let cache = self.config.tool_cache.dir.as_ref().ok_or_else(|| {
                DriverError::Unavailable("this placement has no verified program cache".into())
            })?;
            let path = std::fs::canonicalize(cache.join(&tool.name).join(&tool.version)).map_err(
                |_| {
                    DriverError::NotFound(
                        "the provider's program build is absent from the verified cache".into(),
                    )
                },
            )?;
            let source = self.config.host_path(&path).to_string_lossy().into_owned();
            let target = format!("/opt/orch/tools/{}", tool.name);
            if !parent_host
                .mounts
                .as_ref()
                .into_iter()
                .flatten()
                .any(|mount| {
                    mount.read_only == Some(true)
                        && mount.source.as_deref() == Some(source.as_str())
                        && mount.target.as_deref() == Some(target.as_str())
                })
            {
                return Err(DriverError::InvalidSpec(
                    "the selected build is not this provider unit's installed program".into(),
                ));
            }
            mounts.push(crate::mounts::bind(&source, &target, true));
        }
        host.mounts = Some(mounts);
        let prepared = async {
            let (private, admission) = self
                .prepare_work_egress(&name, spec.work, system, &spec.endpoints)
                .await?;
            env.extend(private.variables());
            restricted::restrict_host(&mut host, &private, false);
            let mut labels: HashMap<String, String> = crate::units::driver_labels(&self.config)
                .into_iter()
                .collect();
            labels.extend([
                (LABEL_SYSTEM.into(), system.clone()),
                (LABEL_KIND.into(), "provider".into()),
                (LABEL_RUNTIME_ID.into(), spec.work.to_string()),
                (LABEL_WORK.into(), spec.work.to_string()),
                (LABEL_INPUT.into(), input.clone()),
                (LABEL_PROVIDER.into(), spec.provider.runtime_id.to_string()),
                (LABEL_DEADLINE.into(), spec.deadline.to_string()),
                (restricted::LABEL_EGRESS.into(), "restricted".into()),
            ]);
            let outward = restricted::ensure_outward(
                &self.daemon,
                &self.config,
                &name,
                spec.work,
                Some(system),
            )
            .await?;
            let connector = self
                .config
                .tool_cache
                .connector_dir
                .as_ref()
                .ok_or_else(|| {
                    DriverError::Unsupported("the trusted proxy has no installed program".into())
                })?;
            let connector = self
                .config
                .host_path(connector)
                .to_string_lossy()
                .into_owned();
            restricted::ensure_proxy(
                &self.daemon,
                &self.config,
                &private,
                &restricted::ProxyMaking {
                    unit: &name,
                    image: &image_id,
                    connector_dir: &connector,
                    outward: &outward.name,
                    gateway_entry: &crate::networks::gateway_entry(&self.config),
                    system: Some(system),
                },
            )
            .await?;
            restricted::start_proxy(&self.daemon, &self.config, &name, self.clock.as_ref()).await?;
            let network = restricted::apply(
                &self.config,
                &name,
                restricted::Reach::Restricted,
                admission,
                self.clock.as_ref(),
            )
            .await?;
            if network.state != orch_driver_contract::network::NetworkReadiness::Ready {
                return Err(DriverError::Unavailable(network.reason.unwrap_or_else(
                    || "the work unit's provider-only network is unavailable".into(),
                )));
            }
            let seconds = spec
                .deadline
                .get()
                .signed_duration_since(self.clock.now())
                .num_seconds();
            if seconds <= 0 {
                return Err(DriverError::Timeout);
            }
            let created = self
                .daemon
                .docker()
                .create_container(
                    Some(CreateContainerOptionsBuilder::default().name(&name).build()),
                    ContainerCreateBody {
                        image: Some(image_id),
                        user: Some("0:0".into()),
                        working_dir: Some("/".into()),
                        // The unprivileged program cannot signal the root watchdog. Its exit ends the whole unit.
                        entrypoint: Some(vec!["/bin/sleep".into()]),
                        cmd: Some(vec![seconds.to_string()]),
                        env: Some(
                            env.into_iter()
                                .map(|(key, value)| format!("{key}={value}"))
                                .collect(),
                        ),
                        labels: Some(labels),
                        host_config: Some(host),
                        ..ContainerCreateBody::default()
                    },
                )
                .await
                .map_err(map_error)?;
            let unit = UnitRef {
                reference: created.id,
                name: name.clone(),
                runtime_id: spec.work,
                kind: RuntimeKind::Provider,
            };
            let dir = restricted::unit_dir(&self.config, &unit.name)?;
            let prepared = async {
                self.daemon
                    .docker()
                    .start_container(&unit.reference, None::<StartContainerOptions>)
                    .await
                    .map_err(map_error)?;
                self.daemon
                    .put_archive(&unit.reference, SCRATCH, archive)
                    .await?;
                crate::egress::files::write_json(&dir, PREPARED, &input)
                    .await
                    .map_err(|_| {
                        DriverError::Unavailable("the prepared work could not be retained".into())
                    })
            }
            .await;
            prepared?;
            Ok(unit)
        }
        .await;
        match prepared {
            Ok(unit) => Ok(unit),
            Err(error) => {
                if let Err(cleanup) = self
                    .cleanup_work_preparation(&name, spec.work, system)
                    .await
                {
                    tracing::warn!(%cleanup, "a refused capability unit still has owned preparation to remove");
                }
                Err(error)
            }
        }
    }
    async fn cleanup_work_preparation(
        &self,
        name: &str,
        work: orch_types::Id,
        system: &str,
    ) -> Result<(), DriverError> {
        let owns = |labels: &HashMap<String, String>| {
            crate::units::is_driver_label_set(labels)
                && labels.get(LABEL_SYSTEM).map(String::as_str) == Some(system)
        };
        for object in [
            name.to_owned(),
            restricted::proxy_name(name),
            format!("{name}-addresses"),
        ] {
            let Some(container) = self.daemon.container(&object).await? else {
                continue;
            };
            let labels = container
                .config
                .as_ref()
                .and_then(|config| config.labels.as_ref());
            let ours = labels.is_some_and(|labels| {
                owns(labels)
                    && if object == name {
                        labels.get(LABEL_WORK) == Some(&work.to_string())
                    } else {
                        labels.get(restricted::LABEL_PROXY_OF).map(String::as_str) == Some(name)
                    }
            });
            if !ours {
                return Err(DriverError::Busy(
                    "an unrelated object holds a work preparation name; it was left unchanged"
                        .into(),
                ));
            }
            let id = container.id.ok_or_else(|| {
                DriverError::Unavailable("an owned preparation has no immutable identity".into())
            })?;
            self.daemon
                .docker()
                .remove_container(
                    &id,
                    Some(
                        bollard::query_parameters::RemoveContainerOptionsBuilder::default()
                            .force(true)
                            .build(),
                    ),
                )
                .await
                .map_err(map_error)?;
        }
        for name in [
            restricted::network_name(name),
            crate::networks::egress_network_name(name),
        ] {
            let network = match self.daemon.docker().inspect_network(&name, None).await {
                Ok(network) => network,
                Err(error) if crate::client::status_of(&error) == Some(404) => continue,
                Err(error) => return Err(map_error(error)),
            };
            if network.labels.as_ref().is_none_or(|labels| !owns(labels)) {
                return Err(DriverError::Busy(
                    "an unrelated network holds a work preparation name; it was left unchanged"
                        .into(),
                ));
            }
            let id = network.id.ok_or_else(|| {
                DriverError::Unavailable("an owned network has no immutable identity".into())
            })?;
            crate::networks::remove(&self.daemon, &id).await?;
        }
        let dir = restricted::unit_dir(&self.config, name)?;
        match tokio::fs::remove_dir_all(dir).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(DriverError::Unavailable(
                "the work's private proxy directory still needs removal".into(),
            )),
        }
    }
}
