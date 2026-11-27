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

impl Frame<'_> {
    /// The owned native command is shared receipt evidence for every result codec.
    pub async fn command(&self, conn: &mut Conn) -> Result<Id, EngineError> {
        command(conn, self.helper, self.turn, self.machine).await
    }
}

/// Actual physical command evidence, shared by native text and structured result readers.
pub(super) async fn command(
    conn: &mut Conn,
    helper: Id,
    turn: Id,
    machine: Option<Id>,
) -> Result<Id, EngineError> {
    // The machine's turn stream and the command mirror advance independently. A crash
    // can lose the handoff checkpoint even though the same machine has reported its turn.
    // Only one matching admitted start can be identified from the turn stream alone.
    let command: Option<Id> = sqlx::query_scalar(
        "WITH starts AS (
            SELECT id, state, created_at FROM commands
            WHERE serial_key=$1 AND machine_id=$2 AND kind IN ('turn.start','turn.start.fenced')
              AND COALESCE(body->>'turn_id',body#>>'{turn,turn_id}')=$3::uuid::text
              AND COALESCE(body->>'thread_id',body#>>'{turn,thread_id}')=$4::uuid::text
        ) SELECT id FROM starts
        WHERE state IN ('sent','accepted','succeeded','failed')
           OR ((SELECT count(*) FROM starts)=1 AND EXISTS (
               SELECT 1 FROM turns t JOIN segments s ON s.id=t.segment_id AND s.thread_id=t.thread_id
               WHERE t.id=$3 AND t.thread_id=$4 AND s.machine_id=$2 AND t.started_at IS NOT NULL))
        ORDER BY created_at DESC,id DESC LIMIT 1",
    )
    .bind(SerialKey::Thread(helper).wire())
    .bind(machine)
    .bind(turn)
    .bind(helper)
    .fetch_optional(conn)
    .await
    .map_err(orch_db::DbError::from)?;
    command.ok_or(EngineError::StaleToolSource)
}

/// Evidence returned only by installed code reading an actual owned native operation.
pub(super) struct Receipt {
    pub command: Id,
    pub result: CapabilityResult,
}

#[async_trait]
pub(super) trait Codec: Send + Sync {
    fn support(&self) -> ExecutionSupport;
    /// Whether this consumer's native tools require a disposable, provider-only unit.
    fn requires_unit(&self) -> bool {
        false
    }
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
    codecs.register(Arc::new(super::text::Text))?;
    codecs.register(Arc::new(super::ask::Dialogue))?;
    Ok(codecs)
}
