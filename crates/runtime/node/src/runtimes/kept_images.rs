//! Kept-image commands, streamed movement and epoch-checked sweeps that protect live units and archived ownership.

use super::*;
use orch_driver_contract::kept::{KeepEstimate, KeptImage};
use orch_link_contract::commands::{
    BlobRef, NodeKeepFiles, NodeKeptImage, NodeKeptImageExport, NodeKeptImport, NodeKeptSweep,
};

// An image archive uses the same content-directory lifetime as a moved runtime's state archive.
const KEPT_ARCHIVE: &str = "kept-image.tar";

fn journal(error: crate::ledger::LedgerError) -> CommandError {
    CommandError::new(ErrorCode::Internal, error.to_string())
}

impl Runtimes {
    pub async fn estimate_kept_files(&self, runtime: Id) -> Result<KeepEstimate, CommandError> {
        let serial = self.lock_of(runtime);
        let _held = serial.lock().await;
        let (host, record) = self.require(runtime).await?;
        host.contract()
            .estimate_keep(&record.unit)
            .await
            .map_err(|error| failure(&error))
    }

    pub async fn keep_unit_files(&self, body: NodeKeepFiles) -> Result<KeptImage, CommandError> {
        let me = self.clone();
        // Losing a caller never lets a rollback restart the container while its commit is still
        // reading the stopped filesystem. This task owns the runtime lock until it finishes.
        tokio::spawn(async move { me.keep_unit_files_held(&body).await })
            .await
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?
    }

    async fn keep_unit_files_held(&self, body: &NodeKeepFiles) -> Result<KeptImage, CommandError> {
        let _busy = self.inner.busy.enter(body.runtime_id);
        let serial = self.lock_of(body.runtime_id);
        let _held = serial.lock().await;
        let (host, record) = self.require(body.runtime_id).await?;
        if self.runs(&host, &record).await {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "installed files are kept only after this unit stops",
            ));
        }
        if self
            .inner
            .ledger
            .about(body.runtime_id)
            .and_then(|about| about.subject)
            .and_then(|subject| subject.environment)
            .is_some_and(|environment| environment.id != body.environment_id)
        {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the kept image names another environment",
            ));
        }
        self.inner.ledger.begin_kept_change().map_err(journal)?;
        let image = host
            .contract()
            .keep_files(
                &record.unit,
                body.environment_id,
                body.generation,
                body.capture,
            )
            .await
            .map_err(|error| failure(&error))?;
        self.report_kept_images().await;
        Ok(image)
    }

    pub async fn remove_kept_image(&self, body: &NodeKeptImage) -> Result<(), CommandError> {
        let host = self.driver(&body.driver)?;
        self.inner.ledger.begin_kept_change().map_err(journal)?;
        host.contract()
            .remove_kept_image(&body.image)
            .await
            .map_err(|error| failure(&error))?;
        self.report_kept_images().await;
        Ok(())
    }

    pub async fn export_kept_image(
        &self,
        command: Id,
        body: &NodeKeptImageExport,
    ) -> Result<BlobRef, CommandError> {
        let host = self.driver(&body.driver)?;
        let support = host.describe().kept_file_transfer;
        if !support.supported {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                support
                    .reason
                    .unwrap_or_else(|| "this driver does not transfer kept installed files".into()),
            ));
        }
        let content = self.content()?;
        let directory = content.directory_of(command);
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        let path = directory.join(KEPT_ARCHIVE);
        let result = async {
            host.contract()
                .export_kept_image(&body.image, &path)
                .await
                .map_err(|error| failure(&error))?;
            content.put(command, &path).await
        }
        .await;
        content.finish(command).await;
        result
    }

    pub async fn import_kept_image(
        &self,
        command: Id,
        body: &NodeKeptImport,
    ) -> Result<(), CommandError> {
        let host = self.driver(&body.driver)?;
        let support = host.describe().kept_file_transfer;
        if !support.supported {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                support
                    .reason
                    .unwrap_or_else(|| "this driver does not transfer kept installed files".into()),
            ));
        }
        let content = self.content()?;
        self.inner.ledger.begin_kept_change().map_err(journal)?;
        let result = async {
            let archive = content
                .fetch(command, body.archive.hash, body.archive.size, KEPT_ARCHIVE)
                .await?;
            host.contract()
                .import_kept_image(&body.image, &archive)
                .await
                .map_err(|error| failure(&error))
        }
        .await;
        content.finish(command).await;
        self.report_kept_images().await;
        result
    }

    /// Startup and hourly inventories are answered by the controller's retained generations and active jobs.
    pub async fn report_kept_images(&self) {
        if !self.inner.drivers.failures().is_empty() {
            return;
        }
        let epoch = self.inner.ledger.kept_epoch();
        let mut images = Vec::new();
        let mut systems = self.inner.ledger.systems();
        if !systems.contains(&self.inner.settings.system_name) {
            systems.push(self.inner.settings.system_name.clone());
        }
        for (_, host) in self.inner.drivers.all() {
            for system in &systems {
                match host.contract().kept_images(system).await {
                    Ok(found) => images.extend(found),
                    Err(error) => {
                        tracing::warn!(driver=%host.name(),%error,"the kept-image inventory could not be read");
                        return;
                    }
                }
            }
        }
        // An operation started during the look makes this inventory incomplete; a fresh report
        // follows that operation, rather than attributing the old list to the new epoch.
        if self.inner.ledger.kept_epoch() == epoch {
            self.inner.reports.kept_images(epoch, images).await;
        }
    }

    pub async fn sweep_kept_images(&self, body: &NodeKeptSweep) -> Result<(), CommandError> {
        if body.epoch != self.inner.ledger.kept_epoch() {
            self.report_kept_images().await;
            return Ok(());
        }
        let mut protected = Vec::new();
        for record in self.inner.ledger.all() {
            let Some(host) = self.inner.drivers.get(&record.driver) else {
                continue;
            };
            if self.runs(&host, &record).await
                && let Some(image) = self.inner.ledger.kept_for(record.runtime_id)
            {
                protected.push(image);
            }
        }
        let mut systems = self.inner.ledger.systems();
        if !systems.contains(&self.inner.settings.system_name) {
            systems.push(self.inner.settings.system_name.clone());
        }
        let mut changed = false;
        let mut owners = body.owners.clone();
        owners.extend(
            protected
                .iter()
                .map(|image| orch_driver_contract::kept::KeptOwner {
                    environment: image.environment,
                    generation: image.generation,
                    capture: Some(image.capture),
                }),
        );
        for (_, host) in self.inner.drivers.all() {
            for system in &systems {
                let images = host
                    .contract()
                    .kept_images(system)
                    .await
                    .map_err(|error| failure(&error))?;
                for image in images {
                    if self.inner.ledger.kept_epoch() != body.epoch {
                        self.report_kept_images().await;
                        return Ok(());
                    }
                    if protected.iter().any(|held| {
                        held.environment == image.environment
                            && held.generation == image.generation
                            && held.capture == image.capture
                    }) || body.owners.iter().any(|owner| {
                        owner.environment == image.environment
                            && owner.generation == image.generation
                            && owner.capture.is_none_or(|capture| capture == image.capture)
                    }) {
                        continue;
                    }
                    host.contract()
                        .remove_kept_image(&image)
                        .await
                        .map_err(|error| failure(&error))?;
                    changed = true;
                }
                if self.inner.ledger.kept_epoch() != body.epoch {
                    self.report_kept_images().await;
                    return Ok(());
                }
                changed |= host
                    .contract()
                    .sweep_kept_metadata(system, &owners)
                    .await
                    .map_err(|error| failure(&error))?
                    > 0;
            }
        }
        // Cleanup itself advances the epoch so an older repeated command cannot describe the
        // completed inventory. Its report lets the node's page remove the freed byte count.
        if changed {
            self.inner.ledger.begin_kept_change().map_err(journal)?;
            self.report_kept_images().await;
        }
        Ok(())
    }
}
