//! Trusted adapter IO for one confined work unit, with no source Read or caller identity.
use crate::adapter::AdapterError;
use async_trait::async_trait;
pub use orch_driver_contract::work::{
    WorkFile, WorkOutput, WorkOutputStream, WorkProcess, WorkProcessSpec,
};
use std::sync::Arc;

/// The trusted connector owns transport authentication; the bare program receives only files and streams.
#[async_trait]
pub trait ProgramIo: Send + Sync {
    /// Prepare only the selected sign-in, native options and frozen accepted material.
    async fn prepare(&self, files: Vec<WorkFile>) -> Result<(), AdapterError>;
    /// Start an exact retained invocation; uncertainty never starts a replacement.
    async fn launch(&self, process: WorkProcessSpec) -> Result<Arc<dyn WorkProcess>, AdapterError>;
    /// Read only one bounded regular file from this work's disposable scratch.
    async fn read_file(&self, path: &str, max_bytes: u64) -> Result<Vec<u8>, AdapterError>;
    /// End the owned unit without changing its provider or ordinary helper history.
    async fn retire(&self) -> Result<(), AdapterError>;
}
