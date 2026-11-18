//! The bundled node prepares one fixed detached helper; update.sh owns every release step and rollback.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use bollard::Docker;
use bollard::models::{
    ContainerCreateBody, HostConfig, Mount, MountType, RestartPolicy, RestartPolicyNameEnum,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, ListContainersOptionsBuilder, RemoveContainerOptionsBuilder,
};
use orch_api_contract::admin::{ServerUpdateObservation, ServerUpdateStage};
use orch_driver_contract::descriptor::OperationSupport;
use orch_driver_contract::driver::DriverError;
use orch_link_contract::commands::NodeServerUpdate;
use orch_types::Id;
use serde::{Deserialize, Serialize};

const JOB_LABEL: &str = "orch.server-update.job";
const ROOT_LABEL: &str = "orch.server-update.installation";
const RELEASE_LABEL: &str = "orch.server-update.release";
const COMMAND_LABEL: &str = "orch.server-update.command";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    command: Id,
    job: Id,
    release: String,
}

fn unavailable() -> DriverError {
    DriverError::Unsupported(
        "the bundled installation and its host Docker access are required".into(),
    )
}
fn native(error: impl std::fmt::Display) -> DriverError {
    tracing::warn!(%error, "the detached server updater could not be prepared");
    DriverError::Unavailable("the bundled node could not prepare its host updater".into())
}
fn root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("ORCH_INSTALLATION_ROOT")?);
    (cfg!(target_os = "linux")
        && root.is_absolute()
        && root.parent().is_some()
        && root.components().all(|part| {
            matches!(
                part,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        }))
    .then_some(root)
}
fn folder(root: &Path, job: Id) -> PathBuf {
    root.join("runtime/node/server-updates")
        .join(job.to_string())
}
fn current(root: &Path) -> PathBuf {
    root.join("runtime/node/server-updates/current.json")
}
fn name(job: Id) -> String {
    format!("orch-update-{}", job.simple())
}
fn observation(request: &Request) -> ServerUpdateObservation {
    ServerUpdateObservation {
        stage: ServerUpdateStage::Queued,
        reason: None,
        from_version: None,
        to_version: request.release.clone(),
    }
}
fn bind(source: &Path, target: &Path, readonly: bool) -> Mount {
    Mount {
        typ: Some(MountType::BIND),
        source: Some(source.display().to_string()),
        target: Some(target.display().to_string()),
        read_only: Some(readonly),
        ..Default::default()
    }
}
fn helper(root: &Path, image: String, socket: &str, request: &Request) -> ContainerCreateBody {
    let inside = Path::new("/host").join(root.strip_prefix("/").unwrap_or(root));
    ContainerCreateBody {
        image: Some(image),
        user: Some("0:0".into()),
        entrypoint: Some(vec![
            "/usr/sbin/chroot".into(),
            "/host".into(),
            "/bin/bash".into(),
        ]),
        cmd: Some(vec![
            root.join("update.sh").display().to_string(),
            format!("v{}", request.release),
            "--yes".into(),
        ]),
        env: Some(vec![
            format!("ORCH_UPDATE_JOB={}", request.job),
            format!(
                "DOCKER_HOST={}",
                if socket.starts_with("unix://") {
                    socket.to_owned()
                } else {
                    format!("unix://{socket}")
                }
            ),
            format!("TMPDIR={}", folder(root, request.job).join("tmp").display()),
            "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
        ]),
        labels: Some(HashMap::from([
            (JOB_LABEL.into(), request.job.to_string()),
            (ROOT_LABEL.into(), root.display().to_string()),
            (RELEASE_LABEL.into(), request.release.clone()),
            (COMMAND_LABEL.into(), request.command.to_string()),
        ])),
        host_config: Some(HostConfig {
            network_mode: Some("host".into()),
            readonly_rootfs: Some(true),
            cap_drop: Some(vec!["ALL".into()]),
            cap_add: Some(vec![
                "SYS_CHROOT".into(),
                "CHOWN".into(),
                "FOWNER".into(),
                "DAC_OVERRIDE".into(),
                "SETUID".into(),
                "SETGID".into(),
            ]),
            security_opt: Some(vec!["no-new-privileges:true".into()]),
            mounts: Some(vec![
                bind(Path::new("/"), Path::new("/host"), true),
                bind(root, &inside, false),
            ]),
            restart_policy: Some(RestartPolicy {
                name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// The current operation is constant-size metadata, never a walk of the node's workspace or volumes.
pub struct Runner {
    docker: Docker,
    socket: String,
    root: Option<PathBuf>,
    pending: Arc<Mutex<HashSet<Id>>>,
    serial: tokio::sync::Mutex<()>,
}
impl Runner {
    pub fn new(docker: Docker, socket: &str) -> Self {
        let root = root().filter(|_| socket.starts_with('/') || socket.starts_with("unix://"));
        let pending = root
            .as_ref()
            .and_then(|root| std::fs::read(current(root)).ok())
            .and_then(|bytes| serde_json::from_slice::<Request>(&bytes).ok())
            .map(|request| HashSet::from([request.command]))
            .unwrap_or_default();
        Self {
            docker,
            socket: socket.to_owned(),
            root,
            pending: Arc::new(Mutex::new(pending)),
            serial: tokio::sync::Mutex::new(()),
        }
    }
    pub fn support(&self) -> Option<OperationSupport> {
        self.root.as_ref().map(|_| OperationSupport {
            supported: true,
            reason: None,
        })
    }
    async fn source_image(&self, root: &Path) -> Result<String, DriverError> {
        let filters: HashMap<String, Vec<String>> = HashMap::from([(
            "label".into(),
            vec![
                "com.docker.compose.service=node".into(),
                format!("com.docker.compose.project.working_dir={}", root.display()),
            ],
        )]);
        let nodes = self
            .docker
            .list_containers(Some(
                ListContainersOptionsBuilder::default()
                    .filters(&filters)
                    .build(),
            ))
            .await
            .map_err(native)?;
        if nodes.len() != 1 {
            return Err(unavailable());
        }
        let id = nodes
            .first()
            .and_then(|node| node.id.as_deref())
            .ok_or_else(unavailable)?;
        let node = self
            .docker
            .inspect_container(id, None)
            .await
            .map_err(native)?;
        let data = root.join("runtime/node").display().to_string();
        if !node.mounts.as_ref().is_some_and(|mounts| {
            mounts.iter().any(|mount| {
                mount.source.as_deref() == Some(data.as_str())
                    && mount.destination.as_deref() == Some(data.as_str())
            })
        }) {
            return Err(unavailable());
        }
        node.image.ok_or_else(unavailable)
    }
    fn matches(root: &Path, request: &Request, labels: Option<&HashMap<String, String>>) -> bool {
        labels.is_some_and(|labels| {
            labels.get(JOB_LABEL) == Some(&request.job.to_string())
                && labels.get(ROOT_LABEL) == Some(&root.display().to_string())
                && labels.get(RELEASE_LABEL) == Some(&request.release)
        })
    }
    async fn read(root: &Path, request: &Request) -> Result<ServerUpdateObservation, DriverError> {
        let path = folder(root, request.job).join("status");
        let text = match tokio::fs::read_to_string(path).await {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(observation(request));
            }
            Err(error) => return Err(native(error)),
        };
        let fields: HashMap<_, _> = text
            .lines()
            .filter_map(|line| line.split_once('='))
            .collect();
        let Some(stage) = fields.get("stage") else {
            return Err(native("the updater's phase is absent"));
        };
        let to = fields
            .get("to")
            .copied()
            .unwrap_or_default()
            .trim_start_matches('v');
        if !to.is_empty() && to != request.release {
            return Err(native("the updater names another release"));
        }
        Ok(ServerUpdateObservation {
            stage: serde_json::from_value(serde_json::json!(stage)).map_err(native)?,
            reason: fields
                .get("reason")
                .filter(|value| !value.is_empty())
                .map(|value| serde_json::from_value(serde_json::json!(value)))
                .transpose()
                .map_err(native)?,
            from_version: fields
                .get("from")
                .filter(|value| !value.is_empty())
                .map(|value| value.trim_start_matches('v').to_owned()),
            to_version: request.release.clone(),
        })
    }
    pub async fn prepare(
        &self,
        command: Id,
        body: &NodeServerUpdate,
    ) -> Result<ServerUpdateObservation, DriverError> {
        let root = self.root.as_ref().ok_or_else(unavailable)?;
        if body.job_id.is_nil()
            || body.timeout_secs == 0
            || !orch_build_info::release::is_release_version(&body.release)
        {
            return Err(DriverError::InvalidSpec(
                "a server update needs its job and exact release".into(),
            ));
        }
        let _serial = self.serial.lock().await;
        let request = Request {
            command,
            job: body.job_id,
            release: body.release.clone(),
        };
        if let Ok(bytes) = tokio::fs::read(current(root)).await {
            let previous: Request = serde_json::from_slice(&bytes).map_err(native)?;
            if previous.job != request.job && !Self::read(root, &previous).await?.stage.terminal() {
                return Err(DriverError::Busy(
                    "another server update still owns the installation's updater".into(),
                ));
            }
        }
        let observed = Self::read(root, &request).await?;
        if observed.stage.terminal() {
            return Ok(observed);
        }
        let path = folder(root, request.job);
        tokio::fs::create_dir_all(path.join("tmp"))
            .await
            .map_err(native)?;
        let existing = self
            .docker
            .inspect_container(&name(request.job), None)
            .await;
        match existing {
            Ok(unit) => {
                if !Self::matches(
                    root,
                    &request,
                    unit.config
                        .as_ref()
                        .and_then(|config| config.labels.as_ref()),
                ) {
                    return Err(native("the helper name belongs to another operation"));
                }
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {
                let image = self.source_image(root).await?;
                self.docker
                    .create_container(
                        Some(
                            CreateContainerOptionsBuilder::default()
                                .name(&name(request.job))
                                .build(),
                        ),
                        helper(root, image, &self.socket, &request),
                    )
                    .await
                    .map_err(native)?;
            }
            Err(error) => return Err(native(error)),
        }
        // Both the operation identity and the container exist before the durable command answer.
        let bytes = serde_json::to_vec(&request).map_err(native)?;
        let temporary = current(root).with_extension("new");
        tokio::fs::write(&temporary, bytes).await.map_err(native)?;
        tokio::fs::File::open(&temporary)
            .await
            .map_err(native)?
            .sync_all()
            .await
            .map_err(native)?;
        tokio::fs::rename(&temporary, current(root))
            .await
            .map_err(native)?;
        tokio::fs::File::open(root.join("runtime/node/server-updates"))
            .await
            .map_err(native)?
            .sync_all()
            .await
            .map_err(native)?;
        if !observed.stage.terminal() {
            self.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(command);
        }
        Ok(observed)
    }
    /// Constant-size identity of a prepared helper, for recovery against the native journal.
    pub fn pending(&self) -> Option<Id> {
        let request: Request =
            serde_json::from_slice(&std::fs::read(current(self.root.as_ref()?)).ok()?).ok()?;
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&request.command)
            .then_some(request.command)
    }
    /// The native journal calls this only after recording and sending the command's answer.
    pub fn answered(&self, command: Id) {
        if !self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&command)
        {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let docker = self.docker.clone();
        let pending = Arc::clone(&self.pending);
        tokio::spawn(async move {
            let start = async {
                let bytes = tokio::fs::read(current(&root)).await.map_err(native)?;
                let request: Request = serde_json::from_slice(&bytes).map_err(native)?;
                if request.command != command {
                    return Ok::<(), DriverError>(());
                }
                let unit = docker
                    .inspect_container(&name(request.job), None)
                    .await
                    .map_err(native)?;
                if !Self::matches(
                    &root,
                    &request,
                    unit.config
                        .as_ref()
                        .and_then(|config| config.labels.as_ref()),
                ) {
                    return Err(unavailable());
                }
                if unit.state.as_ref().and_then(|state| state.running) == Some(true) {
                    return Ok(());
                }
                match docker.start_container(&name(request.job), None).await {
                    Ok(())
                    | Err(bollard::errors::Error::DockerResponseServerError {
                        status_code: 304,
                        ..
                    }) => Ok(()),
                    Err(error) => Err(native(error)),
                }
            }
            .await;
            if let Err(error) = start {
                tracing::warn!(%error, "the exact prepared server updater will be retried after its journal proof");
                pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(command);
            }
        });
    }

    pub async fn observe(
        &self,
    ) -> Result<Option<orch_link_contract::events::NodeServerUpdate>, DriverError> {
        let Some(root) = &self.root else {
            return Ok(None);
        };
        let bytes = match tokio::fs::read(current(root)).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(native(error)),
        };
        let request: Request = serde_json::from_slice(&bytes).map_err(native)?;
        let observation = Self::read(root, &request).await?;
        if observation.stage.terminal()
            && let Ok(unit) = self
                .docker
                .inspect_container(&name(request.job), None)
                .await
            && Self::matches(
                root,
                &request,
                unit.config
                    .as_ref()
                    .and_then(|config| config.labels.as_ref()),
            )
            && (folder(root, request.job).join("parked").is_file()
                || unit.state.as_ref().and_then(|state| state.running) != Some(true))
            && let Err(error) = self
                .docker
                .remove_container(
                    &name(request.job),
                    Some(RemoveContainerOptionsBuilder::default().force(true).build()),
                )
                .await
        {
            tracing::warn!(%error, job = %request.job, "the completed updater helper remains for its next cleanup pass");
        }
        Ok(Some(orch_link_contract::events::NodeServerUpdate {
            job_id: request.job,
            command_id: request.command,
            observation,
        }))
    }
}

#[cfg(all(test, target_os = "linux"))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "the native fixture fails by panicking"
)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Made {
        docker: Docker,
        names: Vec<String>,
    }
    impl Drop for Made {
        fn drop(&mut self) {
            let docker = self.docker.clone();
            let names = self.names.clone();
            let _ = std::thread::spawn(move || {
                if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    runtime.block_on(async move {
                        for name in names {
                            let _ = docker
                                .remove_container(
                                    &name,
                                    Some(
                                        RemoveContainerOptionsBuilder::default()
                                            .force(true)
                                            .build(),
                                    ),
                                )
                                .await;
                        }
                    });
                }
            })
            .join();
        }
    }
    async fn until(mut check: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(90), async {
            while !check() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the detached updater to reach its positive barrier");
    }
    fn at(docker: &Docker, root: &Path) -> Runner {
        Runner {
            docker: docker.clone(),
            socket: "/var/run/docker.sock".into(),
            root: Some(root.to_owned()),
            pending: Arc::new(Mutex::new(HashSet::new())),
            serial: tokio::sync::Mutex::new(()),
        }
    }

    #[tokio::test]
    async fn a_server_update_keeps_one_fixed_helper_after_its_answer_and_outlives_the_node_restart()
    {
        let docker = Docker::connect_with_local_defaults().expect("the native Docker fixture");
        let image = docker
            .inspect_image("orch-dev-harness")
            .await
            .expect("the pinned native harness image")
            .id
            .expect("an immutable fixture image");
        let root = tempfile::Builder::new()
            .prefix("Orch update ")
            .tempdir()
            .expect("a private installation fixture");
        let root = root.path();
        std::fs::create_dir_all(root.join("runtime/node"))
            .expect("the bundled node's own directory");
        std::fs::write(
            root.join("update.sh"),
            r#"#!/bin/bash
set -euo pipefail
root="$(dirname "${BASH_SOURCE[0]}")"
test "$#" = 2
test "$1" = v1.2.3
test "$2" = --yes
record="$root/runtime/node/server-updates/$ORCH_UPDATE_JOB"
printf 'stage=fetching\nreason=\nfrom=v1.2.2\nto=v1.2.3\n' > "$record/status.new"
mv -T "$record/status.new" "$record/status"
: > "$root/started"
while [ ! -f "$root/continue" ]; do sleep .05; done
printf 'stage=updated\nreason=\nfrom=v1.2.2\nto=v1.2.3\n' > "$record/status.new"
mv -T "$record/status.new" "$record/status"
: > "$record/parked"
sleep infinity
"#,
        )
        .expect("a controlled updater that never accesses Docker or credentials");
        let job = orch_types::new_id();
        let command = orch_types::new_id();
        let node_name = format!("orch-test-node-updater-{}", job.simple());
        let _made = Made {
            docker: docker.clone(),
            names: vec![name(job), node_name.clone()],
        };
        docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::default()
                        .name(&node_name)
                        .build(),
                ),
                ContainerCreateBody {
                    image: Some(image),
                    entrypoint: Some(vec!["sleep".into()]),
                    cmd: Some(vec!["infinity".into()]),
                    labels: Some(HashMap::from([
                        ("com.docker.compose.service".into(), "node".into()),
                        (
                            "com.docker.compose.project.working_dir".into(),
                            root.display().to_string(),
                        ),
                    ])),
                    host_config: Some(HostConfig {
                        mounts: Some(vec![bind(
                            &root.join("runtime/node"),
                            &root.join("runtime/node"),
                            false,
                        )]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await
            .expect("a disposable bundled-node stand-in");
        docker
            .start_container(&node_name, None)
            .await
            .expect("the node container");
        let runner = at(&docker, root);
        let request = NodeServerUpdate {
            job_id: job,
            release: "1.2.3".into(),
            timeout_secs: 60,
        };
        assert_eq!(
            runner
                .prepare(command, &request)
                .await
                .expect("prepare")
                .stage,
            ServerUpdateStage::Queued
        );
        assert_eq!(
            runner
                .prepare(command, &request)
                .await
                .expect("the same operation joins")
                .stage,
            ServerUpdateStage::Queued
        );
        assert_eq!(
            docker
                .inspect_container(&name(job), None)
                .await
                .expect("one prepared helper")
                .state
                .expect("its state")
                .running,
            Some(false)
        );
        assert!(
            !root.join("started").exists(),
            "preparation cannot start a host update before its answer"
        );
        let another = NodeServerUpdate {
            job_id: orch_types::new_id(),
            release: "1.2.4".into(),
            timeout_secs: 60,
        };
        assert!(
            matches!(
                runner.prepare(orch_types::new_id(), &another).await,
                Err(DriverError::Busy(_))
            ),
            "another logical job cannot steal an unfinished updater's status record"
        );
        runner.answered(command);
        until(|| root.join("started").is_file()).await;
        docker
            .remove_container(
                &node_name,
                Some(RemoveContainerOptionsBuilder::default().force(true).build()),
            )
            .await
            .expect("stop only this fixture's node");
        drop(runner);
        assert_eq!(
            docker
                .inspect_container(&name(job), None)
                .await
                .expect("the updater survives its node")
                .state
                .expect("its state")
                .running,
            Some(true)
        );
        let returned = at(&docker, root);
        std::fs::write(root.join("continue"), "").expect("release the updater");
        until(|| folder(root, job).join("parked").is_file()).await;
        let result = tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                let observed = returned
                    .observe()
                    .await
                    .expect("the retained updater record")
                    .expect("the same native operation");
                if observed.observation.stage == ServerUpdateStage::Updated {
                    break observed;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the updater to complete");
        assert_eq!(result.job_id, job);
        assert_eq!(result.command_id, command);
        assert_eq!(result.observation.to_version, "1.2.3");
        assert!(
            docker.inspect_container(&name(job), None).await.is_err(),
            "the completed helper is removed without touching the node's state record"
        );
        assert_eq!(
            returned
                .observe()
                .await
                .expect("read after helper removal")
                .expect("retained result"),
            result
        );
    }
}
