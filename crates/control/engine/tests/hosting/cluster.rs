//! A scripted node backed by the cluster-shaped driver rather than unconditional lifecycle successes.

use super::{ScriptedHost, World};
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::runtime::VolumeRole;
use orch_driver_contract::volumes::{StorageInfo, VolumeRequest};
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, Empty, NodeImageEnsureResult,
    NodeRuntimeCreateResult,
};
use orch_testkit::doubles::cluster::{ClusterDriver, ClusterMachine};
use orch_types::Id;
use serde_json::json;
use std::sync::Arc;

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
        host.answer(kind, move |command| {
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
                        CommandResult::NodeRuntimeCreate(NodeRuntimeCreateResult {
                            unit: driver.create(&spec).await.map_err(failure)?,
                        })
                    }
                    Command::NodeRuntimeStart(body) => {
                        let unit = unit(&driver, body.runtime_id)?;
                        driver.start(&unit).await.map_err(failure)?;
                        CommandResult::NodeRuntimeStart(Empty {})
                    }
                    Command::NodeRuntimeStop(body) => {
                        if let Ok(unit) = unit(&driver, body.runtime_id) {
                            driver
                                .stop(&unit, std::time::Duration::ZERO)
                                .await
                                .map_err(failure)?;
                        }
                        CommandResult::NodeRuntimeStop(Empty {})
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
