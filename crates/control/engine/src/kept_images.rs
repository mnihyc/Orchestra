//! Kept-image inventory and cleanup authorized by current units, archived ownership and durable active jobs.

use crate::{Engine, EngineError, commands::Outgoing, ids, unit::Unit};
use orch_driver_contract::kept::{KeepEstimate, KeptImage, KeptOwner};
use orch_link_contract::commands::{NodeKeptSweep, SerialKey};
use orch_link_contract::events::NodeKeptImages;
use orch_types::{Id, Json, Seq};
use serde_json::json;

impl Engine {
    /// A move exports its exact owned image through the same pinned blob lifetime as its volumes.
    pub(crate) async fn carry_kept_image(
        &self,
        job: &crate::jobs::runner::Job,
        old: Id,
        destination: Id,
        image: &KeptImage,
    ) -> Result<(), crate::jobs::runner::Stop> {
        use crate::jobs::runner::{Failure, Stop};
        let mut conn = self.db().acquire().await.map_err(EngineError::from)?;
        let runtime = orch_db::queries::environments::hosting::runtime(&mut conn, old)
            .await?
            .ok_or_else(|| {
                Stop::Failed(Failure::new(
                    "runtime_gone",
                    "the kept-image source is gone",
                ))
            })?;
        let source = orch_db::queries::resources::hosting::host(&mut conn, runtime.host_id)
            .await?
            .ok_or_else(|| {
                Stop::Failed(Failure::new(
                    "no_host",
                    "the kept-image source host is gone",
                ))
            })?;
        let target = orch_db::queries::resources::hosting::host(&mut conn, destination)
            .await?
            .ok_or_else(|| {
                Stop::Failed(Failure::new(
                    "no_host",
                    "the kept-image destination is gone",
                ))
            })?;
        drop(conn);
        let plan = job.plan().await?;
        let archive: orch_link_contract::commands::BlobRef = match plan.get("kept_image_archive") {
            Some(archive) => serde_json::from_value(archive.clone()).map_err(EngineError::from)?,
            None => {
                let value = job
                    .command(
                        Outgoing::new(
                            ids::derived("kept.image.export", &[job.id().as_bytes()]),
                            source.machine_id,
                            SerialKey::Image(image.reference.clone()),
                            &orch_link_contract::commands::NodeKeptImageExport {
                                driver: runtime.driver.clone(),
                                image: image.clone(),
                            },
                        )
                        .map_err(EngineError::from)?,
                    )
                    .await?;
                let archive: orch_link_contract::commands::BlobRef =
                    serde_json::from_value(value).map_err(EngineError::from)?;
                let mut tx = self.db().begin().await.map_err(EngineError::from)?;
                if orch_db::queries::jobs::lock(&mut tx, job.id())
                    .await?
                    .is_some_and(|row| row.plan.get("kept_image_pins").is_none())
                {
                    if orch_db::queries::storage::pin_blobs(&mut tx, &[archive.hash]).await? != 1 {
                        return Err(Stop::Failed(Failure::new(
                            "blob_missing",
                            "the kept-image archive was not stored",
                        )));
                    }
                    orch_db::queries::jobs::merge_plan(
                        &mut tx,
                        job.id(),
                        &json!({"kept_image_archive":archive,"kept_image_pins":[archive.hash]}),
                        job.now(),
                    )
                    .await?;
                }
                tx.commit().await.map_err(orch_db::DbError::from)?;
                archive
            }
        };
        job.command(
            Outgoing::new(
                ids::derived("kept.image.import", &[job.id().as_bytes()]),
                target.machine_id,
                SerialKey::Image(image.reference.clone()),
                &orch_link_contract::commands::NodeKeptImport {
                    driver: runtime.driver,
                    image: image.clone(),
                    archive,
                },
            )
            .map_err(EngineError::from)?,
        )
        .await?;
        Ok(())
    }

    /// Success drops only the old host's copy; rollback drops only the destination's copy.
    pub(crate) async fn cleanup_moved_image(
        &self,
        row: &orch_db::queries::jobs::JobRow,
    ) -> Result<(), EngineError> {
        let Some(image) = row.plan.get("kept_image").filter(|image| !image.is_null()) else {
            return Ok(());
        };
        let image: KeptImage = serde_json::from_value(image.clone())?;
        let recipe: crate::jobs::environment_move::MoveRecipe =
            serde_json::from_value(row.plan.clone())?;
        let committed = row.plan.get("committed").and_then(Json::as_bool) == Some(true);
        let mut conn = self.db().acquire().await?;
        let old =
            orch_db::queries::environments::hosting::runtime(&mut conn, recipe.replacement.old)
                .await?
                .ok_or_else(|| EngineError::unreadable("the move source is gone"))?;
        let host = orch_db::queries::resources::hosting::host(
            &mut conn,
            if committed {
                old.host_id
            } else {
                recipe.destination
            },
        )
        .await?
        .ok_or_else(|| EngineError::unreadable("the move cleanup host is gone"))?;
        drop(conn);
        let command = Outgoing::new(
            ids::derived("kept.image.move.cleanup", &[row.id.as_bytes()]),
            host.machine_id,
            SerialKey::Image(image.reference.clone()),
            &orch_link_contract::commands::NodeKeptImage {
                driver: old.driver,
                image,
            },
        )?;
        let _ = self.ask_bounded(command).await?;
        Ok(())
    }
    /// Archive the stopped writable layer before removing its unit. A retry of this deletion
    /// revision reuses its already recorded image, including after physical unit removal.
    pub(crate) async fn archive_unit_files(
        &self,
        job: &crate::jobs::runner::Job,
        runtime: &orch_db::queries::environments::hosting::RuntimeFull,
    ) -> Result<(), crate::jobs::runner::Stop> {
        use crate::jobs::runner::{Failure, Stop};
        let mut conn = self.db().acquire().await.map_err(EngineError::from)?;
        let host = orch_db::queries::resources::hosting::host(&mut conn, runtime.host_id)
            .await?
            .ok_or_else(|| {
                Stop::Failed(Failure::new(
                    "no_host",
                    "the host keeping the installed files is gone",
                ))
            })?;
        // Whether the unit's own files can be kept is the runtime's, as its class declares it.
        let definition = orch_db::queries::jobs::changes::definition(&mut conn, runtime.id)
            .await
            .map_err(EngineError::from)?;
        if crate::isolation::unit_driver(&host.drivers, &runtime.driver, definition.as_ref())
            .as_ref()
            .and_then(|driver| driver.get("recreate_preserves_files"))
            .and_then(Json::as_bool)
            != Some(true)
        {
            return Ok(());
        }
        let plan = job.plan().await?;
        if let Some(image) = runtime.kept_image.as_ref() {
            let recorded:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE kind='environment_remove' AND environment_id=$1 AND plan->'removal_revision'=$2 AND plan->'kept_image'=$3)")
                .bind(runtime.environment_id).bind(plan.get("removal_revision").cloned().unwrap_or(Json::Null)).bind(image).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
            if recorded {
                drop(conn);
                job.remember(&json!({"kept_image":image})).await?;
                return Ok(());
            }
        }
        let environment = runtime.environment_id.ok_or_else(|| {
            Stop::Failed(Failure::new(
                "internal",
                "a kept image needs its environment",
            ))
        })?;
        drop(conn);
        job.remember(
            &json!({"keep_intent":{"environment":environment,"generation":runtime.generation}}),
        )
        .await?;
        let value = job
            .command(
                Outgoing::new(
                    ids::derived("kept.image.archive", &[job.id().as_bytes()]),
                    host.machine_id,
                    SerialKey::Runtime(runtime.id),
                    &orch_link_contract::commands::NodeKeepFiles {
                        runtime_id: runtime.id,
                        environment_id: environment,
                        generation: i64::from(runtime.generation),
                        capture: job.id(),
                    },
                )
                .map_err(EngineError::from)?,
            )
            .await?;
        let image: KeptImage = serde_json::from_value(value).map_err(EngineError::from)?;
        let mut tx = self.db().begin().await.map_err(EngineError::from)?;
        sqlx::query("UPDATE runtimes SET kept_image=$2 WHERE id=$1")
            .bind(runtime.id)
            .bind(json!(image))
            .execute(&mut *tx)
            .await
            .map_err(orch_db::DbError::from)?;
        orch_db::queries::jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({"kept_image":image}),
            job.now(),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    /// Purge removes exact owned image names along with the retained volumes; failed replies
    /// remain retryable in the same durable purge rather than declaring the files gone.
    pub(crate) async fn purge_unit_images(
        &self,
        job: &crate::jobs::runner::Job,
        runtime: &orch_db::queries::environments::hosting::RuntimeFull,
    ) -> Result<(), crate::jobs::runner::Stop> {
        let mut conn = self.db().acquire().await.map_err(EngineError::from)?;
        let host = orch_db::queries::resources::hosting::host(&mut conn, runtime.host_id)
            .await?
            .ok_or_else(|| {
                crate::jobs::runner::Stop::Failed(crate::jobs::runner::Failure::new(
                    "no_host",
                    "the host keeping the installed files is gone",
                ))
            })?;
        let inventory: Json = sqlx::query_scalar("SELECT kept_images FROM hosts WHERE id=$1")
            .bind(host.id)
            .fetch_one(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
        drop(conn);
        let mut images: Vec<KeptImage> = serde_json::from_value(
            inventory
                .get("images")
                .cloned()
                .unwrap_or_else(|| json!([])),
        )
        .map_err(EngineError::from)?;
        if let Some(image) = runtime
            .kept_image
            .as_ref()
            .and_then(|image| serde_json::from_value::<KeptImage>(image.clone()).ok())
        {
            images.push(image);
        }
        images.sort_by(|a, b| {
            a.reference
                .cmp(&b.reference)
                .then(a.capture.cmp(&b.capture))
        });
        images.dedup_by(|a, b| a.reference == b.reference && a.capture == b.capture);
        for image in images
            .into_iter()
            .filter(|image| Some(image.environment) == runtime.environment_id)
        {
            job.command(
                Outgoing::new(
                    ids::derived(
                        "kept.image.purge",
                        &[
                            job.id().as_bytes(),
                            image.reference.as_bytes(),
                            image.capture.as_bytes(),
                        ],
                    ),
                    host.machine_id,
                    SerialKey::Image(image.reference.clone()),
                    &orch_link_contract::commands::NodeKeptImage {
                        driver: runtime.driver.clone(),
                        image,
                    },
                )
                .map_err(EngineError::from)?,
            )
            .await?;
        }
        Ok(())
    }
    /// Read the old host before presenting a replacement, without stopping or capturing the unit.
    pub(crate) async fn estimate_runtime_files(
        &self,
        machine: Id,
        runtime: Id,
    ) -> Result<Option<KeepEstimate>, EngineError> {
        let command = Outgoing::new(
            orch_types::new_id(),
            machine,
            SerialKey::Runtime(runtime),
            &orch_link_contract::commands::NodeKeepEstimate {
                runtime_id: runtime,
            },
        )?;
        Ok(match self.ask_bounded(command).await? {
            Some(crate::commands::Outcome::Succeeded(value)) => serde_json::from_value(value).ok(),
            _ => None,
        })
    }
    /// Replacement cleanup is retryable through the node inventory even after a lost reply.
    pub(crate) async fn cleanup_recreated_images(
        &self,
        row: &orch_db::queries::jobs::JobRow,
    ) -> Result<(), EngineError> {
        let Some(environment) = row.environment_id else {
            return Ok(());
        };
        let recipe: crate::updates::Recipe = serde_json::from_value(row.plan.clone())?;
        let committed = row.plan.get("committed").and_then(Json::as_bool) == Some(true);
        let (host, driver, images, current) = {
            let mut conn = self.db().acquire().await?;
            let old = orch_db::queries::environments::hosting::runtime(&mut conn, recipe.old)
                .await?
                .ok_or_else(|| EngineError::unreadable("the old kept-image runtime is gone"))?;
            let host = orch_db::queries::resources::hosting::host(&mut conn, old.host_id)
                .await?
                .ok_or_else(|| EngineError::unreadable("the kept-image host is gone"))?;
            let inventory: Json = sqlx::query_scalar("SELECT kept_images FROM hosts WHERE id=$1")
                .bind(host.id)
                .fetch_one(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
            let mut images: Vec<KeptImage> = serde_json::from_value(
                inventory
                    .get("images")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            )?;
            if let Some(image) = old
                .kept_image
                .and_then(|image| serde_json::from_value::<KeptImage>(image).ok())
            {
                images.push(image);
            }
            if let Some(image) = row
                .plan
                .get("kept_image")
                .cloned()
                .and_then(|image| serde_json::from_value::<KeptImage>(image).ok())
            {
                images.push(image);
            }
            let current =
                orch_db::queries::environments::hosting::live_runtime(&mut conn, environment)
                    .await?
                    .and_then(|runtime| runtime.kept_image)
                    .and_then(|image| serde_json::from_value::<KeptImage>(image).ok());
            (host, old.driver, images, current)
        };
        for image in images
            .into_iter()
            .filter(|image| image.environment == environment)
        {
            let remove = if committed {
                current.as_ref().is_none_or(|current| {
                    current.capture != image.capture || current.reference != image.reference
                })
            } else {
                image.capture == row.id
            };
            if !remove {
                continue;
            }
            let body = orch_link_contract::commands::NodeKeptImage {
                driver: driver.clone(),
                image: image.clone(),
            };
            let command = Outgoing::new(
                ids::derived(
                    "kept.image.cleanup",
                    &[
                        row.id.as_bytes(),
                        image.reference.as_bytes(),
                        image.capture.as_bytes(),
                    ],
                ),
                host.machine_id,
                SerialKey::Image(image.reference.clone()),
                &body,
            )?;
            if !matches!(
                self.ask_bounded(command).await?,
                Some(crate::commands::Outcome::Succeeded(_))
            ) {
                tracing::warn!(%environment,image=%image.reference,"kept-image cleanup waits for the node sweep");
            }
        }
        Ok(())
    }

    pub(crate) async fn on_kept_images(
        &self,
        unit: &mut Unit<'_>,
        host: Id,
        report: &NodeKeptImages,
        seq: Seq,
    ) -> Result<(), EngineError> {
        let now = unit.now();
        sqlx::query("UPDATE hosts SET kept_images=$2,updated_at=$3 WHERE id=$1")
            .bind(host)
            .bind(json!(report))
            .bind(now)
            .execute(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
        // Each hourly report can sweep again even when no image changed: ownership may have
        // ended meanwhile. The event sequence deduplicates replay, rather than the image epoch.
        let id = ids::derived(
            "kept.images.sweep",
            &[host.as_bytes(), seq.to_string().as_bytes()],
        );
        sqlx::query("INSERT INTO jobs(id,kind,node_id,state,plan,progress,created_at,updated_at) VALUES($1,'kept_images_sweep',NULL,'queued',$2,$3,$4,$4) ON CONFLICT(id) DO NOTHING")
            .bind(id).bind(json!({"host":host,"epoch":report.epoch}))
            .bind(json!({"phase":"sweep","phases":[{"name":"sweep","weight":100,"state":"pending"}],"percent":0,"files_done":0,"files_total":null,"bytes_done":0,"bytes_total":null,"rate_bps":null,"eta_seconds":null,"message":null}))
            .bind(now).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    pub(crate) async fn kept_owners(
        &self,
        conn: &mut orch_db::Conn,
        host: Id,
    ) -> Result<Vec<KeptOwner>, EngineError> {
        let current:Vec<Json>=sqlx::query_scalar("SELECT r.kept_image FROM runtimes r JOIN environments e ON e.id=r.environment_id WHERE r.host_id=$1 AND r.kept_image IS NOT NULL AND r.successor_of IS NULL AND r.generation=e.generation AND ((r.state NOT IN ('removed','retired') AND e.deleted_at IS NULL) OR (r.state<>'retired' AND e.removal_phase IN ('removing','kept','restoring','purging')))")
            .bind(host).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
        let mut owners: Vec<_> = current
            .into_iter()
            .filter_map(|image| serde_json::from_value::<KeptImage>(image).ok())
            .map(|image| KeptOwner {
                environment: image.environment,
                generation: image.generation,
                capture: Some(image.capture),
            })
            .collect();
        let plans:Vec<(Id,Json)>=sqlx::query_as("SELECT j.id,j.plan FROM jobs j WHERE j.environment_id IS NOT NULL AND j.state IN ('queued','running') AND (j.plan->>'host'=$1::text OR EXISTS(SELECT 1 FROM runtimes r WHERE r.id::text=j.plan->>'old' AND r.host_id=$1) OR EXISTS(SELECT 1 FROM runtimes r WHERE r.environment_id=j.environment_id AND r.host_id=$1 AND r.state NOT IN ('removed','retired')))")
            .bind(host).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
        for (job, plan) in plans {
            if let Some(image) = plan
                .get("kept_image")
                .cloned()
                .and_then(|image| serde_json::from_value::<KeptImage>(image).ok())
            {
                owners.push(KeptOwner {
                    environment: image.environment,
                    generation: image.generation,
                    capture: Some(image.capture),
                });
            }
            // Preserve a commit whose acknowledgment was lost, until its durable job undoes it.
            if let Some(intent) = plan.get("keep_intent")
                && let (Some(environment), Some(generation)) = (
                    intent
                        .get("environment")
                        .and_then(Json::as_str)
                        .and_then(|id| id.parse().ok()),
                    intent.get("generation").and_then(Json::as_i64),
                )
            {
                owners.push(KeptOwner {
                    environment,
                    generation,
                    capture: Some(job),
                });
            }
        }
        Ok(owners)
    }

    pub(crate) async fn kept_sweep_command(
        &self,
        host: Id,
        epoch: u64,
        command: Id,
    ) -> Result<Outgoing, EngineError> {
        let mut conn = self.db().acquire().await?;
        let host_row = orch_db::queries::resources::hosting::host(&mut conn, host)
            .await?
            .ok_or_else(|| EngineError::unreadable("the kept-image host is gone"))?;
        let owners = self.kept_owners(&mut conn, host).await?;
        Ok(Outgoing::new(
            command,
            host_row.machine_id,
            SerialKey::None,
            &NodeKeptSweep { epoch, owners },
        )?)
    }
}

/// The estimate is frozen in the reviewed plan; running work may add files afterwards.
pub(crate) fn recording_note(estimate: Option<KeepEstimate>) -> String {
    match estimate {
        Some(estimate) => format!(
            "About {} MiB is recorded locally in about {} seconds.{}",
            estimate.bytes.saturating_add((1 << 20) - 1) / (1 << 20),
            estimate.seconds,
            if estimate.flatten {
                " This is the tenth layer: the whole filesystem is flattened into one layer."
            } else {
                ""
            }
        ),
        None => {
            "The recording size and time are unavailable while the host cannot measure this unit."
                .into()
        }
    }
}
