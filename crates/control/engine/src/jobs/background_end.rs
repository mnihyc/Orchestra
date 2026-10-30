//! Durable exact native End targets with current authority and truthful partial outcomes.

use async_trait::async_trait;
use orch_access_contract::permissions::Permission;
use orch_api_contract::background::{
    BackgroundEndResult, BackgroundEndResultEntry, BackgroundEndState,
};
use orch_authz::{Act, Caller, Policy, Principal, TokenGrant, nodes};
use orch_db::{DbError, queries::jobs::JobRow};
use orch_link_contract::commands::{SerialKey, SessionBackgroundEnd};
use orch_types::{Id, Json};
use serde_json::json;

use super::runner::{Done, Failure, Job, Phase, Phased, Stop, Undone};
use crate::background::{Identity, Target};
use crate::commands::Outgoing;
use crate::engine::Engine;
use crate::error::EngineError;

/// The existing background mutation job kind.
pub const KIND: &str = "thread_background_end";
/// A single phase is checkpointed after every target; native End concurrency is bounded to one.
pub const PHASES: &[Phase] = &[Phase {
    name: "end",
    weight: 1,
}];

/// Ends the immutable targets of one reviewed request without replaying a roster.
pub struct Ending {
    engine: Engine,
    thread: Id,
    user: Id,
    identity: Identity,
    original: Caller,
    targets: Vec<Target>,
}

impl Ending {
    /// Reads only the mutation identities and original caller from a durable job.
    pub fn of(engine: Engine, row: &JobRow) -> Result<Self, EngineError> {
        let thread = row
            .node_id
            .ok_or_else(|| EngineError::unreadable("the background job has no thread"))?;
        let user = row
            .created_by
            .ok_or_else(|| EngineError::unreadable("the background job has no caller"))?;
        let identity: Identity = serde_json::from_value(row.plan["identity"].clone())?;
        let targets: Vec<Target> = serde_json::from_value(row.plan["targets"].clone())?;
        let original = if row.plan["credential"].is_null() {
            Caller::session(user)
        } else {
            let scopes: Vec<String> =
                serde_json::from_value(row.plan["credential"]["scopes"].clone())?;
            let subtree: Option<Id> =
                serde_json::from_value(row.plan["credential"]["subtree"].clone())?;
            Caller::token(
                user,
                TokenGrant::from_stored(&scopes, subtree).map_err(EngineError::unreadable)?,
            )
        };
        Ok(Self {
            engine,
            thread,
            user,
            identity,
            original,
            targets,
        })
    }

    async fn end(&self, job: &Job, target: &Target) -> Result<BackgroundEndResultEntry, Stop> {
        let mut tx = self.engine.inner.db.begin().await?;
        // The same commit check as an action guard across the exact native mutation. A revocation
        // either precedes this target or refuses its commit; earlier confirmed targets stay ended.
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        let now = self.engine.inner.clock.time();
        let current = if self.identity.token {
            let token: Option<(Vec<String>, Option<Id>)> = sqlx::query_as("SELECT scopes,node_id FROM api_tokens WHERE id=$1 AND user_id=$2 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>$3) FOR SHARE")
                .bind(self.identity.credential_id).bind(self.user).bind(now.get()).fetch_optional(&mut *tx).await.map_err(DbError::from)?;
            match token {
                Some((scopes, subtree)) => Some(Caller::token(
                    self.user,
                    TokenGrant::from_stored(&scopes, subtree)
                        .map_err(|error| Failure::new("authz.denied", error.to_string()))?,
                )),
                None => None,
            }
        } else {
            let valid: Option<Id> = sqlx::query_scalar("SELECT id FROM sessions WHERE id=$1 AND user_id=$2 AND revoked_at IS NULL AND expires_at>$3 FOR SHARE")
                .bind(self.identity.credential_id).bind(self.user).bind(now.get()).fetch_optional(&mut *tx).await.map_err(DbError::from)?;
            valid.map(|_| Caller::session(self.user))
        };
        let mut permitted = current.is_some();
        if let Some(current) = current {
            // Both current and originally captured narrowing apply; editing a token cannot expand a job.
            for caller in [current, self.original.clone()] {
                let decision = match Principal::load(&mut tx, caller, now).await {
                    Ok(principal) => {
                        nodes::check(
                            &mut tx,
                            &principal,
                            self.thread,
                            Permission::ThreadInteract,
                            Act::Use,
                            Policy::default(),
                        )
                        .await
                    }
                    Err(error) => Err(error),
                };
                permitted &= decision.is_ok_and(|decision| decision.is_allowed());
            }
        }
        let outcome = if !permitted {
            BackgroundEndResultEntry {
                id: target.id.clone(),
                state: BackgroundEndState::Unknown,
                reason: Some("permission was lost before this command was ended".to_owned()),
                item_id: target.item_id,
            }
        } else {
            let generation: Option<(Option<Id>, Option<String>)> = sqlx::query_as(
                "SELECT machine_id,native_session_ref FROM segments WHERE id=$1 AND thread_id=$2",
            )
            .bind(target.segment_id)
            .bind(self.thread)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DbError::from)?;
            if generation
                != Some((
                    Some(target.machine_id),
                    Some(target.native_session_ref.clone()),
                ))
            {
                BackgroundEndResultEntry {
                    id: target.id.clone(),
                    state: BackgroundEndState::Unknown,
                    reason: Some("the captured native session generation changed".to_owned()),
                    item_id: target.item_id,
                }
            } else {
                let id = crate::ids::derived(
                    "background.end",
                    &[job.id().as_bytes(), target.id.as_bytes()],
                );
                let outgoing = Outgoing::new(
                    id,
                    target.machine_id,
                    SerialKey::Thread(self.thread),
                    &SessionBackgroundEnd {
                        thread_id: self.thread,
                        segment_id: target.segment_id,
                        command_id: target.command_id.clone(),
                    },
                )
                .map_err(|error| Failure::new("internal", error.to_string()))?;
                match job.command(outgoing).await {
                    Ok(value) => match serde_json::from_value::<
                        orch_link_contract::background::NativeBackgroundEnd,
                    >(value)
                    {
                        Ok(native) if native.id == target.command_id => BackgroundEndResultEntry {
                            id: target.id.clone(),
                            state: serde_json::from_value(
                                serde_json::to_value(native.state).map_err(EngineError::from)?,
                            )
                            .map_err(EngineError::from)?,
                            reason: native.reason,
                            item_id: target.item_id,
                        },
                        _ => BackgroundEndResultEntry {
                            id: target.id.clone(),
                            state: BackgroundEndState::Unknown,
                            reason: Some(
                                "the native reply did not confirm this exact command".to_owned(),
                            ),
                            item_id: target.item_id,
                        },
                    },
                    Err(Stop::Lost) => return Err(Stop::Lost),
                    Err(error) => BackgroundEndResultEntry {
                        id: target.id.clone(),
                        state: BackgroundEndState::Unknown,
                        reason: Some(
                            match error {
                                Stop::Cancelled => {
                                    "the job was cancelled before the command's end was confirmed"
                                }
                                _ => "the native command's end could not be confirmed",
                            }
                            .to_owned(),
                        ),
                        item_id: target.item_id,
                    },
                }
            }
        };
        crate::audit_facts::person(&mut tx,self.user,self.thread,"thread.background_end",json!({"job_id":job.id(),"id":target.id,"segment_id":target.segment_id,"turn_id":target.turn_id,"item_id":target.item_id,"outcome":outcome.state}),self.engine.inner.clock.now()).await?;
        // Checkpoint outcome and its audit together. Recovery asks the native command under the same
        // id if the transaction was lost, and the ordinary command journal refuses a second mutation.
        let mut results: Vec<BackgroundEndResultEntry> = job
            .plan()
            .await?
            .get("outcomes")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(EngineError::from)?
            .unwrap_or_default();
        if !results.iter().any(|result| result.id == outcome.id) {
            results.push(outcome.clone());
        }
        orch_db::queries::jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({"outcomes":results}),
            job.now(),
        )
        .await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(outcome)
    }
}

#[async_trait]
impl Phased for Ending {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        if phase != "end" {
            return Err(
                Failure::new("internal", "an unknown background phase was requested").into(),
            );
        }
        // A confirmed process end cannot be undone; cancellation stops at acceptance of this phase.
        job.pass_commit().await?;
        let results: Vec<BackgroundEndResultEntry> = job
            .plan()
            .await?
            .get("outcomes")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(EngineError::from)?
            .unwrap_or_default();
        let mut completed = results.len();
        for target in &self.targets {
            job.check()?;
            if results.iter().any(|result| result.id == target.id) {
                continue;
            }
            self.end(job, target).await?;
            completed += 1;
            job.progress(completed as f64 / self.targets.len().max(1) as f64, None)
                .await?;
        }
        Ok(Done::Finished)
    }

    async fn undo(&self, _job: &Job, _why: &Stop) -> Undone {
        Undone::Partly("confirmed command ends cannot be undone".to_owned())
    }

    async fn result(&self, job: &Job) -> Option<Json> {
        let results = serde_json::from_value(job.plan().await.ok()?["outcomes"].clone()).ok()?;
        serde_json::to_value(BackgroundEndResult { results }).ok()
    }
}
