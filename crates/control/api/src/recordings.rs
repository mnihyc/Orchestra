//! The last output of a person's shells put away in the store of content with the values of the environment's secrets taken out, and read back.
//!
//! A machine that closes a shell uploads what it kept of the shell's output (at most a few hundred
//! kibibytes of the end of it) to the store of content, and the engine hands the content on to the place
//! the program gave it ([`orch_engine::shells::Recordings`]). This is that place. Where the setting
//! at the shell's place says that secrets are masked, the values of the secrets the environment's
//! commands find are taken out of the text, each replaced by a word that says so; what is put away is
//! then the masked text, a blob of its own, and the machine's upload is left alone for the grace it has
//! as any content nothing points at. Where the text holds none of them, the machine's upload is what the
//! shell's line points at, and nothing is written. Secrets that cannot be opened (the program has no
//! key for them) stop the recording from being kept at all, since keeping it unmasked is what masking
//! is for.

use async_trait::async_trait;
use orch_api_contract::terminals::TERMINAL_RECORDING_MAX_BYTES;
use orch_blob_contract::store::{BlobError, BlobStore, read_all, stream_of};
use orch_db::Db;
use orch_db::secrets::MasterKey;
use orch_engine::shells::{RecordingFault, Recordings, mask_secrets};
use orch_link_contract::commands::BlobRef;
use orch_settings::secrets::{self, Owner};
use orch_types::{Hash, Id};
use std::sync::Arc;

/// Where the last output of shells is put away.
pub struct KeptOutput {
    store: Arc<dyn BlobStore>,
    db: Db,
    master: Option<MasterKey>,
}

impl KeptOutput {
    /// The place that keeps recordings in `store`, and opens the secrets of environments (which are
    /// stored in `db`, sealed) with `master` where the program has one.
    #[must_use]
    pub fn new(store: Arc<dyn BlobStore>, db: Db, master: Option<MasterKey>) -> Self {
        Self { store, db, master }
    }

    /// The values of the secrets the commands of `environment` find; none where it has none.
    async fn secrets_of(&self, environment: Id) -> Result<Vec<String>, RecordingFault> {
        let fault = |error: &dyn std::fmt::Display| RecordingFault(error.to_string());
        let mut conn = self.db.acquire().await.map_err(|error| fault(&error))?;
        let owner = Owner::Environment(environment);
        let named = secrets::list(&mut conn, owner)
            .await
            .map_err(|error| fault(&error))?;
        if named.is_empty() {
            return Ok(Vec::new());
        }
        let Some(master) = &self.master else {
            return Err(RecordingFault(
                "the secrets of the environment cannot be opened to take them out of the output"
                    .to_owned(),
            ));
        };
        Ok(secrets::open_resolved(&mut conn, master, owner)
            .await
            .map_err(|error| fault(&error))?
            .into_iter()
            .map(|opened| opened.value)
            .collect())
    }
}

/// Gives the engine the place that keeps the last output of shells, once, at start: in `store`, with the
/// secrets of environments opened from `db` with `master` where there is one. False where it had one
/// already.
pub fn install(store: Arc<dyn BlobStore>, db: Db, master: Option<MasterKey>) -> bool {
    orch_engine::shells::install_recordings(Arc::new(KeptOutput::new(store, db, master)))
}

#[async_trait]
impl Recordings for KeptOutput {
    async fn keep(
        &self,
        environment: Id,
        kept: BlobRef,
        mask: bool,
    ) -> Result<BlobRef, RecordingFault> {
        if !mask {
            return Ok(kept);
        }
        if kept.size > TERMINAL_RECORDING_MAX_BYTES {
            return Err(RecordingFault(
                "the output is longer than may be put away".to_owned(),
            ));
        }
        let values = self.secrets_of(environment).await?;
        if values.is_empty() {
            return Ok(kept);
        }
        let read = self
            .store
            .get(&kept.hash, None)
            .await
            .map_err(|error| RecordingFault(error.to_string()))?;
        let text = read_all(read.body)
            .await
            .map_err(|error| RecordingFault(error.to_string()))?;
        let values: Vec<&str> = values.iter().map(String::as_str).collect();
        let masked = mask_secrets(&text, &values);
        if masked.as_slice() == text.as_ref() {
            return Ok(kept);
        }
        let hash = Hash::of(&masked);
        let size = i64::try_from(masked.len()).unwrap_or(i64::MAX);
        self.store
            .put(&hash, Some(size), stream_of(vec![masked.into()]))
            .await
            .map_err(|error| RecordingFault(error.to_string()))?;
        Ok(BlobRef { hash, size })
    }

    async fn read(&self, hash: &Hash) -> Result<Option<Vec<u8>>, RecordingFault> {
        match self.store.get(hash, None).await {
            Ok(read) => Ok(Some(
                read_all(read.body)
                    .await
                    .map_err(|error| RecordingFault(error.to_string()))?
                    .to_vec(),
            )),
            Err(BlobError::NotFound(_)) => Ok(None),
            Err(error) => Err(RecordingFault(error.to_string())),
        }
    }
}
