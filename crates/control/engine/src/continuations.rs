//! Authored prompt versions retain native prefixes and select execution by one durable guarded job.

pub(crate) mod admission;
pub mod projection;
pub mod recipe;

use crate::{engine::Engine, error::EngineError, inputs::InputError, unit::Unit};
use orch_api_contract::threads::{
    ThreadContinuationAccepted, ThreadContinuationActivateBody, ThreadPromptVersionCreateBody,
};
use orch_authz::Principal;
use orch_types::Id;

/// A request refusal never substitutes a copied/filtered conversation for native preparation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("There is no such thread, input or continuation.")]
    NotFound,
    #[error("Only the person who sent these words can edit them.")]
    NotAuthor,
    #[error("{0}")]
    Invalid(&'static str),
    #[error("{0}")]
    Conflict(&'static str),
    #[error("{0}")]
    Unsupported(&'static str),
    #[error(transparent)]
    Input(#[from] InputError),
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<orch_db::DbError> for Error {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}
impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl Engine {
    /// Admission and its durable identities commit before any native/blob/workspace wait.
    pub async fn create_prompt_version_guarded(
        &self,
        mut tx: orch_db::Tx,
        principal: Principal,
        thread: Id,
        input: Id,
        body: ThreadPromptVersionCreateBody,
    ) -> Result<ThreadContinuationAccepted, Error> {
        let request = recipe::Request {
            kind: recipe::Kind::Edit,
            source_continuation_id: body.source_continuation_id,
            before_input_id: Some(input),
            target_continuation_id: None,
            expected: recipe::Expected {
                active_continuation_id: body.expected_active_continuation_id,
                execution_revision: body.expected_execution_revision,
                queue_revision: body.expected_queue_revision,
            },
            active_turn: admission::running(body.active_turn),
            content: Some(body.content),
        };
        let accepted = self
            .accept_continuation(&mut tx, &principal, thread, &body.client_id, request)
            .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.kick();
        self.wake(thread);
        Ok(accepted)
    }

    /// Selecting retained work does not replay its old prompt or consume its pending queue.
    pub async fn activate_continuation_guarded(
        &self,
        mut tx: orch_db::Tx,
        principal: Principal,
        thread: Id,
        target: Id,
        body: ThreadContinuationActivateBody,
    ) -> Result<ThreadContinuationAccepted, Error> {
        let request = recipe::Request {
            kind: recipe::Kind::Activate,
            source_continuation_id: target,
            before_input_id: None,
            target_continuation_id: Some(target),
            expected: recipe::Expected {
                active_continuation_id: body.expected_active_continuation_id,
                execution_revision: body.expected_execution_revision,
                queue_revision: body.expected_queue_revision,
            },
            active_turn: admission::running(body.active_turn),
            content: body.content,
        };
        let accepted = self
            .accept_continuation(&mut tx, &principal, thread, &body.client_id, request)
            .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.kick();
        self.wake(thread);
        Ok(accepted)
    }

    async fn accept_continuation(
        &self,
        tx: &mut orch_db::Tx,
        principal: &Principal,
        thread: Id,
        client: &str,
        request: recipe::Request,
    ) -> Result<ThreadContinuationAccepted, Error> {
        // This helper owns no native activity. The surrounding public method commits the receipt.
        let mut unit = Unit::new(tx, principal.now().get());
        let accepted =
            admission::accept(self, &mut unit, principal, thread, client, request).await?;
        unit.finish().await?;
        Ok(accepted)
    }
}
