//! Installed codecs own input and result shapes; the runtime owns no capability-specific branch.
use super::*;
use async_trait::async_trait;
use orch_provider_contract::capabilities::{CapabilityResult, ExecutionSupport, RegistryError};
use std::sync::Arc;

/// Actual physical ownership already checked by the source-bound runtime, never a permission.
pub(super) struct Frame<'a> {
    pub helper: Id,
    pub turn: Id,
    pub machine: Option<Id>,
    pub capability: &'a str,
    pub input: &'a Json,
}

/// Evidence returned only by installed code reading an actual owned native operation.
pub(super) struct Receipt {
    pub command: Id,
    pub result: CapabilityResult,
}

#[async_trait]
pub(super) trait Codec: Send + Sync {
    fn support(&self) -> ExecutionSupport;
    fn prepare(
        &self,
        capability: &str,
        input: &Json,
        media: &[orch_model_contract::generation::GenerationPart],
    ) -> Result<Vec<ContentPart>, EngineError>;
    async fn read(
        &self,
        conn: &mut Conn,
        frame: &Frame<'_>,
    ) -> Result<Option<Receipt>, EngineError>;
}

#[derive(Default)]
pub(super) struct Codecs {
    entries: BTreeMap<(String, String), Arc<dyn Codec>>,
}

impl Codecs {
    pub fn register(&mut self, codec: Arc<dyn Codec>) -> Result<(), RegistryError> {
        let support = codec.support();
        if support.input.is_empty() || support.result.is_empty() {
            return Err(RegistryError::MissingName);
        }
        let key = (support.input, support.result);
        if self.entries.contains_key(&key) {
            return Err(RegistryError::Conflict {
                noun: "codec",
                id: format!("{}/{}", key.0, key.1),
            });
        }
        self.entries.insert(key, codec);
        Ok(())
    }
    pub fn support(&self) -> Vec<ExecutionSupport> {
        self.entries.values().map(|codec| codec.support()).collect()
    }
    pub fn get(&self, required: &ExecutionSupport) -> Result<Arc<dyn Codec>, EngineError> {
        self.entries
            .get(&(required.input.clone(), required.result.clone()))
            .filter(|codec| codec.support().satisfies(required))
            .cloned()
            .ok_or_else(|| {
                EngineError::unreadable("The selected program consumer codec is not installed.")
            })
    }
}

pub(super) fn installed() -> Result<Codecs, RegistryError> {
    let mut codecs = Codecs::default();
    codecs.register(Arc::new(super::image::Image))?;
    Ok(codecs)
}
