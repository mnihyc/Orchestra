//! Measured owned-volume selectors and durable whole-volume carry before a destination unit first mounts its private layers.

use std::collections::BTreeSet;

use orch_api_contract::forks::{FilesHome, FilesPlan, PrivateViewPlan};
use orch_db::queries::{
    environments::hosting as runtimes, jobs, resources::hosting as hosts, storage,
};
use orch_driver_contract::volumes::OwnedVolumeSelector;
use orch_link_contract::commands::{
    ExportedVolume, NodeRuntimeVolumeImport, NodeRuntimeVolumesExport,
    NodeRuntimeVolumesExportResult, NodeRuntimeVolumesMeasure, NodeRuntimeVolumesMeasureResult,
    SelectedVolume, SerialKey,
};
use orch_types::Id;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::commands::{Outcome, Outgoing};
use crate::jobs::runner::{Failure, Job, Stop};
use crate::{Engine, EngineError};

/// The source frozen before destination provisioning, with exactly the reviewed owned selectors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Carry {
    pub source_runtime: Id,
    pub selectors: Vec<OwnedVolumeSelector>,
}

fn fail(message: impl Into<String>) -> Stop {
    Stop::Failed(Failure::new("volume_copy_unavailable", message))
}

/// The private physical layer is over:<mount id> in the controller's ownership ledger.
pub(crate) fn role(selector: OwnedVolumeSelector) -> String {
    match selector {
        OwnedVolumeSelector::Home | OwnedVolumeSelector::FileStates => "home".into(),
        OwnedVolumeSelector::Private(mount) => format!("over:{mount}"),
    }
}

fn selected(
    runtime: &runtimes::RuntimeFull,
    selectors: &[OwnedVolumeSelector],
) -> Result<Vec<SelectedVolume>, EngineError> {
    selectors
        .iter()
        .map(|selector| {
            let value = runtime.volumes.get(role(*selector)).ok_or_else(|| {
                EngineError::unreadable(format!(
                    "the old runtime has no owned volume for {selector}"
                ))
            })?;
            Ok(SelectedVolume {
                selector: *selector,
                volume: serde_json::from_value(value.clone())?,
            })
        })
        .collect()
}

pub(crate) fn carried(files: Option<&FilesPlan>) -> Vec<OwnedVolumeSelector> {
    let Some(files) = files else {
        return Vec::new();
    };
    let mut selectors = Vec::new();
    if files.home.is_some() {
        selectors.push(OwnedVolumeSelector::Home);
    } else if files.file_states.is_some() {
        selectors.push(OwnedVolumeSelector::FileStates);
    }
    selectors.extend(
        files
            .private_views
            .iter()
            .filter(|view| view.carried)
            .map(|view| OwnedVolumeSelector::Private(view.mount.id)),
    );
    selectors
}

pub(crate) async fn check_target(
    engine: &Engine,
    machine: Id,
    driver: &str,
) -> Result<(), EngineError> {
    let mut conn = engine.db().acquire().await?;
    archive_part_bytes(&mut conn).await?;
    drop(conn);
    let command = Outgoing::new(
        orch_types::new_id(),
        machine,
        SerialKey::None,
        &orch_link_contract::commands::NodeVolumeCarryCheck {
            driver: driver.into(),
        },
    )?;
    let Some(Outcome::Succeeded(value)) = engine.ask_bounded(command).await? else {
        return Err(EngineError::unreadable(
            "the target host did not verify whole-volume carry support",
        ));
    };
    let result: orch_link_contract::commands::NodeVolumeCarryCheckResult =
        serde_json::from_value(value)?;
    if result.available {
        Ok(())
    } else {
        Err(EngineError::unreadable(result.reason.unwrap_or_else(
            || "the target cannot carry whole owned volumes".into(),
        )))
    }
}

pub(crate) async fn archive_part_bytes(conn: &mut orch_db::Conn) -> Result<i64, EngineError> {
    let max = orch_settings::get::<i64>(
        conn,
        orch_settings::Place::System,
        orch_settings_contract::registry::keys::STORAGE_MAX_BLOB_BYTES,
    )
    .await?
    .unwrap_or(8 * 1024 * 1024 * 1024);
    if max <= 0 {
        return Err(EngineError::unreadable(
            "the system's blob limit permits no volume archive",
        ));
    }
    Ok(max.min(orch_link_contract::commands::VOLUME_ARCHIVE_PART_MAX_BYTES))
}

/// Measures all private layers in a plan, including those left behind, and an explicitly carried home.
pub(crate) async fn measure(
    engine: &Engine,
    conn: &mut orch_db::Conn,
    environment: Id,
    home: bool,
    by: Id,
    private: &mut [PrivateViewPlan],
) -> Result<Option<FilesHome>, EngineError> {
    measure_inner(engine, conn, environment, home, by, private, false).await
}

/// A move can continue from its record when the source explicitly cannot copy its optional native home.
pub(crate) async fn measure_for_move(
    engine: &Engine,
    conn: &mut orch_db::Conn,
    environment: Id,
    home: bool,
    by: Id,
    private: &mut [PrivateViewPlan],
) -> Result<Option<FilesHome>, EngineError> {
    measure_inner(engine, conn, environment, home, by, private, true).await
}

async fn measure_inner(
    engine: &Engine,
    conn: &mut orch_db::Conn,
    environment: Id,
    home: bool,
    by: Id,
    private: &mut [PrivateViewPlan],
    home_fallback: bool,
) -> Result<Option<FilesHome>, EngineError> {
    let mut selectors = Vec::new();
    if home {
        selectors.push(OwnedVolumeSelector::Home);
    }
    selectors.extend(
        private
            .iter()
            .map(|view| OwnedVolumeSelector::Private(view.mount.id)),
    );
    if selectors.is_empty() {
        return Ok(None);
    }
    let runtime = runtimes::live_runtime(conn, environment)
        .await?
        .ok_or_else(|| {
            EngineError::unreadable("this source has no owned home or private-layer volumes")
        })?;
    let host = hosts::host(conn, runtime.host_id)
        .await?
        .ok_or_else(|| EngineError::unreadable("the old host is gone"))?;
    let mut measured_home = None;
    for selector in selectors {
        let body = json!({"runtime_id":runtime.id,"volumes":selected(&runtime, &[selector])?});
        let subject = json!({"environment_id":environment});
        let kind = orch_link_contract::commands::CommandKind::NodeRuntimeVolumesMeasure;
        let count = if home_fallback && selector == OwnedVolumeSelector::Home {
            engine
                .measure_home_in_background(host.machine_id, kind, body, subject, by)
                .await?
        } else {
            engine
                .measure_in_background(host.machine_id, kind, body, subject, by)
                .await?
        };
        if count.unsupported {
            continue;
        }
        let measured = count
            .result
            .map(serde_json::from_value::<NodeRuntimeVolumesMeasureResult>)
            .transpose()?;
        if measured
            .as_ref()
            .is_some_and(|value| value.volumes.len() != 1 || value.volumes[0].selector != selector)
        {
            return Err(EngineError::unreadable(
                "the old host did not measure exactly the requested selector",
            ));
        }
        let value = measured
            .and_then(|value| {
                value
                    .volumes
                    .into_iter()
                    .find(|volume| volume.selector == selector)
            })
            .map(|volume| volume.measure);
        if value.is_some_and(|value| value.file_count < 0 || value.total_bytes < 0) {
            return Err(EngineError::unreadable(
                "the old host returned an invalid volume measurement",
            ));
        }
        if selector == OwnedVolumeSelector::Home {
            measured_home = Some(FilesHome {
                measurement: Some(count.reference),
                measured: value.is_some(),
                file_count: value.map_or(0, |value| value.file_count),
                total_bytes: value.map_or(0, |value| value.total_bytes),
            });
        } else if let Some(view) = private
            .iter_mut()
            .find(|view| selector == OwnedVolumeSelector::Private(view.mount.id))
        {
            view.measurement = Some(count.reference);
            view.measured = value.is_some();
            view.file_count = value.map_or(0, |value| value.file_count);
            view.total_bytes = value.map_or(0, |value| value.total_bytes);
        }
    }
    Ok(measured_home)
}

/// Measures the fixed store even when the person leaves all other home contents behind.
pub(crate) async fn measure_file_states(
    engine: &Engine,
    conn: &mut orch_db::Conn,
    environment: Id,
    by: Id,
) -> Result<orch_api_contract::forks::FilesPlanFileStates, EngineError> {
    let runtime = runtimes::live_runtime(conn, environment)
        .await?
        .ok_or_else(|| EngineError::unreadable("the owned file-state home is unavailable"))?;
    let host = hosts::host(conn, runtime.host_id)
        .await?
        .ok_or_else(|| EngineError::unreadable("the source host is unavailable"))?;
    let count = engine.measure_in_background(host.machine_id,
        orch_link_contract::commands::CommandKind::NodeRuntimeVolumesMeasure,
        json!({"runtime_id":runtime.id,"volumes":selected(&runtime, &[OwnedVolumeSelector::FileStates])?}),
        json!({"environment_id":environment}), by).await?;
    let Some(result) = count.result else {
        return Ok(orch_api_contract::forks::FilesPlanFileStates {
            measurement: Some(count.reference),
            roots: 0,
            objects: 0,
            total_bytes: 0,
            journals: 0,
            complete: false,
        });
    };
    let result: NodeRuntimeVolumesMeasureResult = serde_json::from_value(result)?;
    if result.volumes.len() != 1 {
        return Err(EngineError::unreadable(
            "the source returned an ambiguous store measurement",
        ));
    }
    let measured = result
        .volumes
        .into_iter()
        .find(|volume| volume.selector == OwnedVolumeSelector::FileStates)
        .and_then(|volume| volume.measure.file_states)
        .filter(|value| {
            value.complete
                && value.roots >= 0
                && value.objects >= 0
                && value.total_bytes >= 0
                && value.journals >= 0
        })
        .ok_or_else(|| {
            EngineError::unreadable("the source did not measure every persistent store part")
        })?;
    let referenced: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM snapshots WHERE environment_id=$1 AND storage='environment'
        AND state='complete' AND (expires_at IS NULL OR expires_at > $2)",
    )
    .bind(environment)
    .bind(engine.inner.clock.now())
    .fetch_one(conn)
    .await
    .map_err(orch_db::DbError::from)?;
    if measured.roots < referenced {
        return Err(EngineError::unreadable(
            "the measured store is missing recorded roots",
        ));
    }
    let mut value: orch_api_contract::forks::FilesPlanFileStates = measured.into();
    value.measurement = Some(count.reference);
    Ok(value)
}

/// A candidate's imported store is checked before the original host can be released.
pub(crate) async fn verify_file_states(
    job: &Job,
    runtime: Id,
    expected: &orch_api_contract::forks::FilesPlanFileStates,
) -> Result<(), Stop> {
    if !expected.complete {
        return Err(fail("an unmeasured file-state store cannot be committed"));
    }
    let mut conn = job
        .engine()
        .db()
        .acquire()
        .await
        .map_err(EngineError::from)?;
    let target = runtimes::runtime(&mut conn, runtime)
        .await?
        .ok_or_else(|| fail("the candidate runtime disappeared"))?;
    let host = hosts::host(&mut conn, target.host_id)
        .await?
        .ok_or_else(|| fail("the candidate host disappeared"))?;
    let volumes = selected(&target, &[OwnedVolumeSelector::FileStates])?;
    drop(conn);
    let value = job
        .command(
            Outgoing::new(
                crate::ids::derived("move.file-states.volume.verify", &[job.id().as_bytes()]),
                host.machine_id,
                SerialKey::Runtime(runtime),
                &NodeRuntimeVolumesMeasure {
                    work: None,
                    runtime_id: runtime,
                    volumes,
                },
            )
            .map_err(EngineError::from)?,
        )
        .await?;
    let result: NodeRuntimeVolumesMeasureResult =
        serde_json::from_value(value).map_err(EngineError::from)?;
    let measure = result
        .volumes
        .into_iter()
        .find(|volume| volume.selector == OwnedVolumeSelector::FileStates)
        .and_then(|volume| volume.measure.file_states)
        .ok_or_else(|| fail("the candidate did not verify its imported file-state store"))?;
    if !measure.complete || measure.roots < expected.roots || measure.journals < expected.journals {
        return Err(fail(
            "recorded roots or recovery journals are missing on the candidate; the source is kept",
        ));
    }
    job.remember(&json!({ "file_states_verified": true }))
        .await?;
    Ok(())
}

/// Extra file count and logical bytes carried outside the workspace selection.
pub(crate) fn totals(home: Option<&FilesHome>, private: &[PrivateViewPlan]) -> (i64, i64) {
    let mut files = home.map_or(0, |home| home.file_count);
    let mut bytes = home.map_or(0, |home| home.total_bytes);
    for view in private.iter().filter(|view| view.carried && view.measured) {
        files = files.saturating_add(view.file_count);
        bytes = bytes.saturating_add(view.total_bytes);
    }
    (files, bytes)
}

/// Ensured targets are still unused here. Export once, pin once, and import before create ever runs.
pub(crate) async fn apply(
    job: &Job,
    target: &runtimes::RuntimeFull,
    target_machine: Id,
    environment: Id,
) -> Result<(), Stop> {
    let plan = job.plan().await?;
    let Some(carry) = plan.get("volume_carry") else {
        return Ok(());
    };
    let carry: Carry = serde_json::from_value(carry.clone()).map_err(EngineError::from)?;
    if carry.selectors.is_empty() {
        return Ok(());
    }
    let target_volumes = selected(target, &carry.selectors)?;
    let archives: Vec<ExportedVolume> = match plan.get("volume_archives") {
        Some(archives) => serde_json::from_value(archives.clone()).map_err(EngineError::from)?,
        None => {
            let mut conn = job
                .engine()
                .db()
                .acquire()
                .await
                .map_err(EngineError::from)?;
            let source = runtimes::runtime(&mut conn, carry.source_runtime)
                .await?
                .ok_or_else(|| fail("the reviewed source runtime is gone"))?;
            let host = hosts::host(&mut conn, source.host_id)
                .await?
                .ok_or_else(|| fail("the old host is gone"))?;
            let volumes = selected(&source, &carry.selectors)?;
            let part_bytes = archive_part_bytes(&mut conn).await?;
            drop(conn);
            if volumes.iter().zip(&target_volumes).any(|(from, to)| {
                from.volume.name == to.volume.name && host.machine_id == target_machine
            }) {
                return Err(fail("the copy would overwrite its source volume"));
            }
            let value = job
                .command(
                    Outgoing::new(
                        crate::ids::derived("node.runtime.volumes.export", &[job.id().as_bytes()]),
                        host.machine_id,
                        SerialKey::Runtime(source.id),
                        &NodeRuntimeVolumesExport {
                            work: None,
                            runtime_id: source.id,
                            volumes,
                            part_bytes,
                        },
                    )
                    .map_err(EngineError::from)?,
                )
                .await?;
            let exported: NodeRuntimeVolumesExportResult =
                serde_json::from_value(value).map_err(EngineError::from)?;
            let requested = carry.selectors.iter().copied().collect::<BTreeSet<_>>();
            if exported.volumes.len() != requested.len()
                || exported
                    .volumes
                    .iter()
                    .map(|volume| volume.selector)
                    .collect::<BTreeSet<_>>()
                    != requested
            {
                return Err(fail(
                    "the source did not export every promised whole volume",
                ));
            }
            let pins = exported
                .volumes
                .iter()
                .flat_map(|volume| volume.archive.parts.iter().map(|part| part.hash))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let mut tx = job.engine().db().begin().await.map_err(EngineError::from)?;
            let locked = jobs::lock(&mut tx, job.id())
                .await?
                .ok_or_else(|| fail("the carry job is gone"))?;
            if locked.plan.get("volume_archives").is_none() {
                storage::pin_blobs(&mut tx, &pins).await?;
                jobs::merge_plan(
                    &mut tx,
                    job.id(),
                    &json!({"volume_archives": exported.volumes, "volume_archive_pins":pins}),
                    job.now(),
                )
                .await?;
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
            exported.volumes
        }
    };
    for selected in target_volumes {
        let archive = archives
            .iter()
            .find(|archive| archive.selector == selected.selector)
            .ok_or_else(|| fail("a carried selector has no archive"))?
            .archive
            .clone();
        let selector = selected.selector.to_string();
        job.command(
            Outgoing::new(
                crate::ids::derived(
                    "node.runtime.volume.import",
                    &[job.id().as_bytes(), selector.as_bytes()],
                ),
                target_machine,
                SerialKey::Runtime(target.id),
                &NodeRuntimeVolumeImport {
                    work: None,
                    runtime_id: target.id,
                    driver: target.driver.clone(),
                    environment_id: environment,
                    provider_id: (target.kind == "provider").then_some(environment),
                    selected,
                    archive,
                },
            )
            .map_err(EngineError::from)?,
        )
        .await?;
    }
    Ok(())
}

impl Engine {
    /// Releases transient carry archives once on success, failure or cancellation, independently of job kind.
    pub(crate) async fn after_volume_carry(&self, row: &jobs::JobRow) {
        if row.plan.get("volume_archive_pins").is_none() {
            return;
        }
        let released = async {
            let mut tx = self.db().begin().await?;
            let Some(current) = jobs::lock(&mut tx, row.id).await? else {
                return Ok::<_, EngineError>(());
            };
            let pins: Vec<orch_types::Hash> = serde_json::from_value(
                current
                    .plan
                    .get("volume_archive_pins")
                    .cloned()
                    .unwrap_or(json!([])),
            )?;
            if !pins.is_empty() {
                storage::unpin_blobs(&mut tx, &pins, self.inner.clock.now()).await?;
                jobs::merge_plan(
                    &mut tx,
                    row.id,
                    &json!({"volume_archive_pins":[]}),
                    self.inner.clock.now(),
                )
                .await?;
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
            Ok::<_, EngineError>(())
        }
        .await;
        if let Err(error) = released {
            tracing::warn!(%error, "whole-volume carry pins could not be released");
        }
    }
}
