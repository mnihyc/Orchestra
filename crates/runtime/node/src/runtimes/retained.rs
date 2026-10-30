//! Frozen owned-volume verification and purge independent of the removed unit.

use super::Runtimes;
use crate::drivers::failure;
use orch_driver_contract::runtime::RuntimeSpec;
use orch_driver_contract::volumes::VolumeRequest;
use orch_link_contract::commands::{CommandError, ErrorCode, NodeRuntimeRemove, NodeVolumeRemove};
use orch_types::Id;
use std::collections::BTreeMap;

fn invalid(why: &str) -> CommandError {
    CommandError::new(ErrorCode::Conflict, why)
}

impl Runtimes {
    /// Removes the unit alone, or independently removes only verified frozen owned references.
    pub async fn remove_retained(&self, body: &NodeRuntimeRemove) -> Result<(), CommandError> {
        if body.keep_volumes {
            return self.remove(body.runtime_id).await;
        }
        let serial = self.lock_of(body.runtime_id);
        let _one = serial.lock().await;
        let mut requests = Vec::new();
        for reference in &body.volume_refs {
            let runtime = reference
                .get("runtime_id")
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse::<Id>().ok());
            if runtime != Some(body.runtime_id) {
                return Err(invalid("the frozen reference is for another runtime"));
            }
            let environment = reference
                .get("environment_id")
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse::<Id>().ok())
                .ok_or_else(|| invalid("the frozen reference has no owner"))?;
            let name = reference
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| invalid("the frozen reference has no volume name"))?;
            let role = reference
                .get("role")
                .and_then(|v| v.as_str())
                .ok_or_else(|| invalid("the frozen reference has no role"))?;
            // Moves use a twelve-hex runtime suffix to keep source and target volumes distinct.
            // The exact environment/system labels are still verified below by the driver.
            let ordinary = |prefix: &str| {
                let base = format!("{prefix}-{environment}");
                name == base
                    || name
                        .strip_prefix(&format!("{base}-"))
                        .is_some_and(|suffix| {
                            suffix.len() == 12
                                && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
                        })
            };
            let owned = match role {
                "workspace" => ordinary("orch-ws"),
                "home" => ordinary("orch-home"),
                "state" => ordinary("orch-state"),
                private if private.starts_with("over:") => {
                    name.starts_with(&format!("orch-over-{environment}-"))
                }
                _ => false,
            };
            if !owned {
                return Err(invalid(
                    "a frozen reference does not name an owned environment volume",
                ));
            }
            let driver = reference
                .get("driver")
                .and_then(|v| v.as_str())
                .ok_or_else(|| invalid("the frozen reference has no driver"))?;
            let request = VolumeRequest {
                name: name.to_owned(),
                storage: reference
                    .get("storage")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                labels: BTreeMap::from([
                    (
                        "orch.system".to_owned(),
                        self.inner.settings.system_name.clone(),
                    ),
                    ("orch.environment_id".to_owned(), environment.to_string()),
                ]),
            };
            let host = self.driver(driver)?;
            host.extras()
                .volume_exists(&request)
                .await
                .map_err(|e| failure(&e))?;
            if self
                .inner
                .ledger
                .using_volume(name)
                .iter()
                .any(|record| record.runtime_id != body.runtime_id)
            {
                return Err(invalid("another unit still references the kept volume"));
            }
            requests.push((host, driver.to_owned(), request));
        }
        if let Some((host, record)) = self.find(body.runtime_id).await? {
            host.contract()
                .remove(&record.unit)
                .await
                .map_err(|e| failure(&e))?;
            self.forget(body.runtime_id).await;
        }
        for (host, driver, request) in requests {
            self.inner
                .volumes
                .remove(
                    &NodeVolumeRemove {
                        driver,
                        name: request.name.clone(),
                        storage: request.storage.clone(),
                        labels: request.labels.clone(),
                    },
                    &self.inner.settings.system_name,
                )
                .await?;
            if host
                .extras()
                .volume_exists(&request)
                .await
                .map_err(|e| failure(&e))?
            {
                return Err(invalid("the host did not confirm volume absence"));
            }
        }
        Ok(())
    }

    /// Refuses restoration from missing or foreign files before a new unit can create empty replacements.
    pub(super) async fn verify_kept(&self, spec: &RuntimeSpec) -> Result<(), CommandError> {
        let driver = self.driver(&spec.driver)?;
        let owner = spec
            .labels
            .get("orch.environment_id")
            .ok_or_else(|| invalid("the restoring unit has no environment owner"))?;
        let volumes = spec.volumes.values().map(|v| (&v.name, &v.storage)).chain(
            spec.mounts
                .iter()
                .filter_map(|m| m.layer.as_ref())
                .map(|v| (&v.name, &v.storage)),
        );
        for (name, storage) in volumes {
            if ![
                format!("orch-ws-{owner}"),
                format!("orch-home-{owner}"),
                format!("orch-state-{owner}"),
            ]
            .contains(name)
                && !name.starts_with(&format!("orch-over-{owner}-"))
            {
                return Err(invalid(
                    "the restoring spec names files outside this environment's owned volumes",
                ));
            }
            let request = VolumeRequest {
                name: name.clone(),
                storage: storage.clone(),
                labels: BTreeMap::from([
                    (
                        "orch.system".to_owned(),
                        self.inner.settings.system_name.clone(),
                    ),
                    ("orch.environment_id".to_owned(), owner.to_owned()),
                ]),
            };
            if !driver
                .extras()
                .volume_exists(&request)
                .await
                .map_err(|e| failure(&e))?
            {
                return Err(CommandError::new(
                    ErrorCode::NotFound,
                    "a kept volume is missing; no empty replacement was made",
                ));
            }
        }
        Ok(())
    }
}
