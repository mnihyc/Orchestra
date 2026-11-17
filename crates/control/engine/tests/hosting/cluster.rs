//! A scripted node backed by the cluster-shaped driver rather than unconditional lifecycle successes.

use super::{ScriptedHost, World, lock};
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::runtime::VolumeRole;
use orch_driver_contract::volumes::{StorageInfo, VolumeRequest};
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, Empty, ErrorCode, NodeImageEnsureResult,
    NodeRuntimeCreateResult,
};
use orch_link_contract::native_target::{NativeTarget, RuntimeSource};
use orch_testkit::doubles::cluster::{ClusterDriver, ClusterMachine};
use orch_types::Id;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

fn failure(error: DriverError) -> CommandError {
    CommandError {
        code: error.code().into(),
        detail: error.to_string(),
        retryable: error.retryable(),
        details: None,
    }
}

/// Installs real driver lifecycle effects behind the engine test's ordinary machine transport.
pub async fn install(world: &World, host: &ScriptedHost) -> Arc<ClusterDriver> {
    let driver = Arc::new(
        ClusterDriver::new(
            vec![
                ClusterMachine {
                    id: "member-a".into(),
                    cpus: 4,
                    memory_mb: 8192,
                },
                ClusterMachine {
                    id: "member-b".into(),
                    cpus: 8,
                    memory_mb: 16384,
                },
            ],
            vec![StorageInfo {
                name: "fast".into(),
                default: true,
                capacity_gb: 200,
                free_gb: 150,
                private_views: false,
            }],
        )
        .with_archive_codec(Arc::new(orch_testkit::doubles::cluster::VolumeCodec)),
    );
    let capacity = driver.host_resources().unwrap();
    sqlx::query("UPDATE hosts SET drivers=$2, capacity=$3, storages=$4 WHERE id=$1")
        .bind(host.id)
        .bind(json!({(driver.describe().name):driver.describe()}))
        .bind(
            json!({"cpus":capacity.cpus,"memory_mb":capacity.memory_mb,"disk_gb":capacity.disk_gb}),
        )
        .bind(json!(driver.storages().unwrap()))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let sources = Arc::new(Mutex::new(HashMap::<Id, RuntimeSource>::new()));
    for kind in [
        CommandKind::NodeImageEnsure,
        CommandKind::NodeVolumeEnsure,
        CommandKind::NodeVolumeRemove,
        CommandKind::NodeRuntimeCreate,
        CommandKind::NodeRuntimeStart,
        CommandKind::NodeRuntimeStop,
        CommandKind::NodeRuntimeRemove,
    ] {
        let driver = driver.clone();
        let sources = sources.clone();
        host.answer_with_context(kind, move |context,command| {
            futures::executor::block_on(async {
                Ok(match command {
                    Command::NodeImageEnsure(body) => {
                        let image = driver.ensure_image(&body.spec).await.map_err(failure)?;
                        CommandResult::NodeImageEnsure(NodeImageEnsureResult {
                            programs: Default::default(),
                            digest: image.digest,
                            built: None,
                        })
                    }
                    Command::NodeVolumeEnsure(body) => {
                        driver
                            .ensure_volume(&VolumeRequest {
                                name: body.name.clone(),
                                storage: body.storage.clone(),
                                labels: body.labels.clone(),
                            })
                            .await
                            .map_err(failure)?;
                        CommandResult::NodeVolumeEnsure(Empty {})
                    }
                    Command::NodeVolumeRemove(body) => {
                        driver.remove_volume(&body.name).await.map_err(failure)?;
                        CommandResult::NodeVolumeRemove(Empty {})
                    }
                    Command::NodeRuntimeCreate(body) => {
                        // Enrollment is simulated by the hosting harness; this driver never consumes the sealed token.
                        let mut value = serde_json::to_value(&body.0).map_err(|error| {
                            failure(DriverError::InvalidSpec(error.to_string()))
                        })?;
                        value["enroll_token"] = json!("scripted-driver-token");
                        let spec: orch_driver_contract::runtime::RuntimeSpec =
                            serde_json::from_value(value).map_err(|error| {
                                failure(DriverError::InvalidSpec(error.to_string()))
                            })?;
                        let unit = driver.create(&spec).await.map_err(failure)?;
                        let source = *lock(&sources).entry(spec.runtime_id)
                            .or_insert_with(||RuntimeSource {incarnation:Some(orch_types::new_id())});
                        CommandResult::NodeRuntimeCreate(NodeRuntimeCreateResult {source,unit})
                    }
                    Command::NodeRuntimeStart(body) => {
                        let unit = unit(&driver,body.runtime_id)?;
                        let actual = lock(&sources).get(&body.runtime_id).copied().ok_or_else(||CommandError::new(ErrorCode::NotFound,"the fixture native source is absent"))?;
                        let target = match context.native_target.as_ref() {Some(NativeTarget::Lifecycle(target))=>Some(target),_=>None};
                        if target.is_some_and(|target|!target.matches_source(actual.incarnation)) {
                            return Err(CommandError::new(ErrorCode::Conflict,"the fixture native source changed"));
                        }
                        let reused = driver.inspect(&unit).await.map_err(failure)?.state == orch_driver_contract::units::UnitStateKind::Running;
                        driver.start(&unit).await.map_err(failure)?;
                        let source = if reused {actual} else {RuntimeSource {incarnation:Some(target.and_then(|target|target.opening).unwrap_or_else(orch_types::new_id))}};
                        lock(&sources).insert(body.runtime_id,source);
                        CommandResult::NodeRuntimeStart(orch_link_contract::commands::NodeRuntimeStartResult {source,reused})
                    }
                    Command::NodeRuntimeStop(body) => {
                        let unit = unit(&driver,body.runtime_id)?;
                        let source = lock(&sources).get(&body.runtime_id).copied().ok_or_else(||CommandError::new(ErrorCode::NotFound,"the fixture native source is absent"))?;
                        if context.native_target.as_ref().is_some_and(|target|matches!(target,NativeTarget::Lifecycle(target) if !target.matches_source(source.incarnation))) {
                            return Err(CommandError::new(ErrorCode::Conflict,"the fixture native source changed"));
                        }
                        driver.stop(&unit,std::time::Duration::ZERO).await.map_err(failure)?;
                        CommandResult::NodeRuntimeStop(orch_link_contract::commands::NodeRuntimeStopResult::Stopped {source})
                    }
                    Command::NodeRuntimeRemove(body) => {
                        if let Ok(unit) = unit(&driver, body.runtime_id) {
                            driver.remove(&unit).await.map_err(failure)?;
                        }
                        if !body.keep_volumes {
                            for volume in &body.volume_refs {
                                let name = volume
                                    .get("name")
                                    .and_then(serde_json::Value::as_str)
                                    .ok_or_else(|| {
                                    failure(DriverError::InvalidSpec(
                                        "the frozen volume reference has no name".into(),
                                    ))
                                })?;
                                driver.remove_volume(name).await.map_err(failure)?;
                            }
                        }
                        CommandResult::NodeRuntimeRemove(Empty {})
                    }
                    _ => unreachable!(),
                })
            })
        });
    }
    driver
}

fn unit(
    driver: &ClusterDriver,
    runtime: Id,
) -> Result<orch_driver_contract::units::UnitRef, CommandError> {
    let spec = driver.unit_spec(runtime).ok_or_else(|| {
        failure(DriverError::NotFound(
            "the cluster has no such runtime".into(),
        ))
    })?;
    Ok(orch_driver_contract::units::UnitRef {
        reference: format!("unit:{runtime}"),
        name: spec.name,
        runtime_id: runtime,
        kind: spec.kind,
    })
}

/// The workspace volume selected by the driver's actual launch request.
pub fn workspace(driver: &ClusterDriver, runtime: Id) -> String {
    driver.unit_spec(runtime).unwrap().volumes[&VolumeRole::Workspace]
        .name
        .clone()
}
