//! Managed future inputs use the API's existing scheduled-input producer and due sweep.
use super::definitions::{Code, Refusal};
use super::{Answer, CallContext, definition_of, setting};
use crate::{Engine, EngineError};
use async_trait::async_trait;
use orch_db::{Conn, queries::threads::ThreadRow};
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::keys;
use orch_types::Json;
use std::sync::{Arc, PoisonError};

/// Whether this exact thread may be offered the managed tool now.
pub async fn offered(conn: &mut Conn, thread: &ThreadRow) -> Result<bool, EngineError> {
    if thread.side_of.is_some() {
        return Ok(false);
    }
    let wanted = [definition_of(keys::THREADS_AGENT_SCHEDULES)?];
    let snapshot = Snapshot::load(conn, Place::Node(thread.node_id), Wanted::Keys(&wanted)).await?;
    Ok(setting::<String>(&snapshot, keys::THREADS_AGENT_SCHEDULES)?.as_deref() == Some("on"))
}

/// The host supplies the same store, timing, controller checks and due runner as people's schedules.
#[async_trait]
pub trait ScheduleTools: Send + Sync {
    /// Called only inside the source-checked Orch tool dispatch; writes recheck source on their transaction.
    async fn call(&self, call: &CallContext, thread: &ThreadRow) -> Result<Json, Refusal>;
}

impl Engine {
    /// Attach the API's scheduled-input producer without retaining its application through the engine.
    pub fn attach_schedule_tools(&self, tools: Arc<dyn ScheduleTools>) {
        *self
            .inner
            .schedule_tools
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(tools);
    }

    pub(super) async fn schedule_call(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
    ) -> Result<Answer, EngineError> {
        let tools = self
            .inner
            .schedule_tools
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(tools) = tools else {
            return Ok(Refusal::new(
                Code::Unsupported,
                "Managed schedules are not available in this process.",
            )
            .into());
        };
        Ok(match tools.call(call, thread).await {
            Ok(value) => Answer::Done(value),
            Err(refusal) => Answer::Refused(refusal),
        })
    }
}
