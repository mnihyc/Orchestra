//! Disposable networkless driver unit for one immutable cached build.

use std::path::Path;
use std::time::Duration;

use bollard::Docker;
use bollard::models::{ContainerCreateBody, HostConfig, Mount, MountType};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, LogsOptionsBuilder, RemoveContainerOptionsBuilder,
    WaitContainerOptionsBuilder,
};
use futures::StreamExt;
use orch_driver_contract::driver::DriverError;
use orch_driver_docker::DockerConfig;
use orch_link_contract::commands::{NodeToolConformance, NodeToolConformanceResult};
use tokio_util::sync::CancellationToken;

/// Native evidence is short; a bounded read rejects a broken runner that emits an unbounded stream.
const MAX_EVIDENCE_BYTES: usize = 4 * 1024 * 1024;
/// Stable private paths inside this one disposable unit.
const RUNNER: &str = "/opt/orch-check/node";
const BUILD: &str = "/opt/orch-check/build";
/// The unprivileged account carried by the release's environment base image.
const CHECK_USER: &str = "1000:1000";

struct OwnedUnit {
    docker: Docker,
    name: String,
    removed: bool,
}

impl Drop for OwnedUnit {
    fn drop(&mut self) {
        if self.removed {
            return;
        }
        let docker = self.docker.clone();
        let name = self.name.clone();
        // An aborted command still removes only the unit this invocation created.
        tokio::spawn(async move {
            let _ = docker
                .remove_container(
                    &name,
                    Some(RemoveContainerOptionsBuilder::default().force(true).build()),
                )
                .await;
        });
    }
}

fn bind(source: &Path, target: &str) -> Mount {
    Mount {
        source: Some(source.display().to_string()),
        target: Some(target.to_owned()),
        typ: Some(MountType::BIND),
        read_only: Some(true),
        ..Default::default()
    }
}

fn error(error: impl std::fmt::Display) -> DriverError {
    DriverError::Internal(format!("native check unit: {error}"))
}

/// Creates, waits for and always removes the driver-owned unit; no image is pulled here.
///
/// # Errors
/// The driver could not create the isolated unit, the runner timed out, or its evidence was unreadable.
pub async fn run(
    docker: &Docker,
    config: &DockerConfig,
    request: &NodeToolConformance,
    program: &Path,
    cancel: &CancellationToken,
) -> Result<NodeToolConformanceResult, DriverError> {
    if !cfg!(target_os = "linux") {
        return Err(DriverError::Unsupported(
            "native harness checks need a Linux isolation driver".to_owned(),
        ));
    }
    let runner = config
        .live_helper_binary
        .clone()
        .or_else(|| std::env::current_exe().ok())
        .ok_or_else(|| DriverError::Unsupported("no node runner executable".to_owned()))?;
    if !orch_driver_docker::live_mounts::static_binary(&runner) {
        return Err(DriverError::Unsupported(
            "native checks need the shipped static node runner".to_owned(),
        ));
    }
    let image = config
        .conformance_image
        .clone()
        .unwrap_or_else(|| format!("orch-env-base:{}", env!("ORCH_RELEASE_VERSION")));
    docker.inspect_image(&image).await.map_err(|_| {
        DriverError::Unsupported(format!("native checks need the preloaded image {image}"))
    })?;
    let fallback = program
        .parent()
        .ok_or_else(|| error("the cached executable has no directory"))?;
    let directory = config
        .tool_cache
        .dir
        .as_ref()
        .map(|dir| dir.join(&request.name).join(&request.version))
        .unwrap_or_else(|| fallback.to_owned());
    let canonical = std::fs::canonicalize(program).map_err(error)?;
    let root = std::fs::canonicalize(&directory).map_err(error)?;
    let relative = canonical
        .strip_prefix(root)
        .map_err(|_| error("the executable points outside its verified build"))?;
    let name = format!("orchprobe-conform-{}", orch_types::new_id().simple());
    let mut labels: std::collections::HashMap<_, _> = config.labels.clone().into_iter().collect();
    labels
        .entry("orch.system".to_owned())
        .or_insert_with(|| "harness-check".to_owned());
    labels.insert("orch.check".to_owned(), name.clone());
    let body = ContainerCreateBody {
        image: Some(image),
        user: Some(CHECK_USER.to_owned()),
        entrypoint: Some(vec![RUNNER.to_owned()]),
        cmd: Some(vec![
            "__harness-conformance".to_owned(),
            "--kind".to_owned(),
            request.name.clone(),
            "--version".to_owned(),
            request.version.clone(),
            "--program".to_owned(),
            Path::new(BUILD).join(relative).display().to_string(),
            "--case-timeout-secs".to_owned(),
            request.case_timeout_secs.to_string(),
            "--scratch".to_owned(),
            "/tmp/orch-check".to_owned(),
        ]),
        env: Some(vec![
            "HOME=/tmp/orch-check/home".to_owned(),
            "PATH=/usr/local/bin:/usr/bin:/bin".to_owned(),
        ]),
        labels: Some(labels),
        host_config: Some(HostConfig {
            network_mode: Some("none".to_owned()),
            readonly_rootfs: Some(true),
            cap_drop: Some(vec!["ALL".to_owned()]),
            security_opt: Some(vec!["no-new-privileges:true".to_owned()]),
            mounts: Some(vec![
                bind(&config.host_path(&runner), RUNNER),
                bind(&config.host_path(&directory), BUILD),
            ]),
            tmpfs: Some(std::collections::HashMap::from([(
                "/tmp".to_owned(),
                "rw,nosuid,nodev,mode=1777".to_owned(),
            )])),
            ..Default::default()
        }),
        ..Default::default()
    };
    docker
        .create_container(
            Some(CreateContainerOptionsBuilder::default().name(&name).build()),
            body,
        )
        .await
        .map_err(error)?;
    let mut owned = OwnedUnit {
        docker: docker.clone(),
        name: name.clone(),
        removed: false,
    };
    let work = async {
        docker.start_container(&name, None).await.map_err(error)?;
        let mut wait = docker.wait_container(
            &name,
            Some(
                WaitContainerOptionsBuilder::default()
                    .condition("not-running")
                    .build(),
            ),
        );
        let ended = wait
            .next()
            .await
            .ok_or_else(|| error("no unit exit result"))?
            .map_err(error)?;
        let mut logs = docker.logs(
            &name,
            Some(
                LogsOptionsBuilder::default()
                    .stdout(true)
                    .stderr(false)
                    .build(),
            ),
        );
        let mut bytes = Vec::new();
        while let Some(next) = logs.next().await {
            let next = next.map_err(error)?;
            if bytes.len().saturating_add(next.as_ref().len()) > MAX_EVIDENCE_BYTES {
                return Err(error("the runner exceeded its evidence bound"));
            }
            bytes.extend_from_slice(next.as_ref());
        }
        if ended.status_code != 0 {
            return Err(error(format!(
                "the runner exited with {}",
                ended.status_code
            )));
        }
        serde_json::from_slice::<NodeToolConformanceResult>(&bytes).map_err(error)
    };
    let result = tokio::select! {
        () = cancel.cancelled() => Err(DriverError::Internal("the native check was cancelled".to_owned())),
        result = tokio::time::timeout(Duration::from_secs(u64::from(request.timeout_secs)), work) => result.unwrap_or(Err(DriverError::Timeout)),
    };
    let removed = docker
        .remove_container(
            &name,
            Some(RemoveContainerOptionsBuilder::default().force(true).build()),
        )
        .await
        .map_err(error);
    owned.removed = removed.is_ok();
    match (result, removed) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(result), Ok(())) => Ok(result),
    }
}
