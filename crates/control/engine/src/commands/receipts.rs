//! Factual native command receipts and the existing import ACK reconciliation.
use super::CommandAuthority;
use crate::{
    EngineError,
    environment_mcp::{ImportCommand, existing},
};
use orch_db::{Conn, queries::link::CommandRow};
use orch_types::Clock;

/// A verified native ACK view. It provides no queue/send authority or raw-envelope extraction.
pub struct McpImportAck {
    source: Box<ImportCommand>,
}
impl McpImportAck {
    /// The closed Move receipt ends an existing source, so its ACK starts in authority mode.
    pub fn changes_authority(&self) -> bool {
        matches!(self.source.as_ref(),ImportCommand::Duplicate(source) if source.input().moving())
    }
    /// The existing native definition lock order, before the canonical successful receipt is stored.
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        self.source.lock_source(conn).await
    }
    /// Reconcile the actual stored successful result through the existing native writer.
    pub async fn reconcile(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<existing::Completion, EngineError> {
        self.source.reconcile(conn, clock).await
    }
}
impl CommandAuthority {
    /// The exact accepted directory coordinates from its verified HTTP-native frame.
    /// This is data only: no credential restoration, new queue, write or result Read.
    pub fn file_mkdir_receipt(
        &self,
        row: &CommandRow,
    ) -> Result<Option<super::FileMkdirInput>, EngineError> {
        match self {
            Self::Http(source) => source.file_mkdir_receipt(row),
            _ => Ok(None),
        }
    }

    /// Verify only immutable native association. This restores no caller or IO authority.
    /// A sealed HTTP channel may carry any native command; older bare channels supply
    /// their own canonical receipt comparison instead of guessing from their kind.
    fn receipt_source<'a>(&'a self, row: &CommandRow) -> Result<Option<&'a Self>, EngineError> {
        match self {
            Self::Http(envelope) => envelope.receipt_native(row),
            Self::McpImport(import) if row.kind == "mcp.import" => {
                let matches = match import.as_ref() {
                    ImportCommand::Creation(source) => source.matches_receipt(row)?,
                    ImportCommand::Existing(source) => source.matches(row)?,
                    ImportCommand::Duplicate(source) => source.matches(row)?,
                };
                Ok(matches.then_some(self))
            }
            Self::FileCopy(source) => Ok(source.matches_receipt(row)?.then_some(self)),
            _ => Ok(None),
        }
    }
    fn import_receipt<'a>(
        &'a self,
        row: &CommandRow,
    ) -> Result<Option<&'a ImportCommand>, EngineError> {
        if row.kind != "mcp.import" {
            return Ok(None);
        }
        let Some(Self::McpImport(import)) = self.receipt_source(row)? else {
            return Ok(None);
        };
        // A wrapper proves its exact frame; preserve the native import comparison too.
        let matches = match import.as_ref() {
            ImportCommand::Creation(source) => source.matches_receipt(row)?,
            ImportCommand::Existing(source) => source.matches(row)?,
            ImportCommand::Duplicate(source) => source.matches(row)?,
        };
        Ok(matches.then_some(import.as_ref()))
    }
    /// Exact completed native evidence, including one sealed HTTP envelope. No new IO.
    /// Completion is factual and does not restore an ended original issuing credential.
    pub fn matches_native_receipt(
        &self,
        expected_native: &Self,
        row: &CommandRow,
    ) -> Result<bool, EngineError> {
        if row.state != "succeeded" {
            return Ok(false);
        }
        // Keep the import's native matching contract, including for its wrapped form.
        if let Self::McpImport(expected) = expected_native {
            return Ok(self
                .import_receipt(row)?
                .is_some_and(|actual| actual == expected.as_ref()));
        }
        Ok(self
            .receipt_source(row)?
            .is_some_and(|actual| actual == expected_native))
    }
    /// The actual immutable file-copy input is data, not its captured caller or a send permit.
    pub fn file_copy_receipt(
        &self,
        row: &CommandRow,
    ) -> Result<Option<super::FileCopyInput>, EngineError> {
        let Some(Self::FileCopy(source)) = self.receipt_source(row)? else {
            return Ok(None);
        };
        if !source.matches_receipt(row)? {
            return Ok(None);
        }
        Ok(Some(source.receipt_input()))
    }
    /// Called only by the authenticated native success-ACK path, before finish_command.
    /// Creation follows its normal filing branch; an existing import gets its opaque reconciler.
    pub fn mcp_import_ack(&self, row: &CommandRow) -> Result<Option<McpImportAck>, EngineError> {
        match self.import_receipt(row)? {
            Some(ImportCommand::Creation(_)) => Ok(None),
            Some(ImportCommand::Duplicate(source)) if source.input().is_issue() => Ok(None),
            Some(source @ (ImportCommand::Existing(_) | ImportCommand::Duplicate(_))) => {
                Ok(Some(McpImportAck {
                    source: Box::new(source.clone()),
                }))
            }
            None => Err(EngineError::unreadable(
                "the acknowledged import has no matching native receipt source",
            )),
        }
    }
}
