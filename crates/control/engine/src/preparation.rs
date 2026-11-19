//! An input's files written before a turn, a lease or a model request exists; an input whose files cannot be had is held, each file named with why, until its author recovers it.
//!
//! Pre-flight checks the oldest ready input before making a turn. Images are always materialized;
//! when native image input is unavailable the model receives their verified paths instead.
//! An input that carries files has them written first: the thread is `starting` with the step `files`
//! and no turn, the input waits with the detail [`PREPARING`], and the supervisor's look at the thread
//! starts the environment where it must, writes the files with the one
//! command the delivery makes, and settles the input: [`FILES_READY`], and pre-flight runs again and
//! makes the turn, its checks asked anew since time passed; or held for its files
//! (`state_detail = 'attachments'`, with an `AttachmentIssue` for each file that could not be written),
//! and pre-flight goes on with the next input. A hold is not retried by the clock or by an unrelated
//! input: its author retries it, removes the files, or a manager changes the model.

use crate::delivery::{self, Delivering, Placement, Slot};
use crate::engine::Engine;
use crate::environments::Standing;
use crate::error::EngineError;
use crate::unit::Unit;
use orch_api_contract::source::{SourceInfo, SourceState};
use orch_api_contract::threads::ContentPart;
use orch_db::queries::environments::thread_engine as environments;
use orch_db::queries::threads::{
    self, HELD_ATTACHMENTS, InputRow, StateChange, ThreadRow, input_state, thread_state,
};
use orch_types::{Id, Json};
use serde_json::json;

/// The `state_detail` of an input whose files are being written before its turn: it waits in its place
/// while the thread is `starting` with the step `files`.
pub const PREPARING: &str = "files";

/// The `state_detail` of an input whose files were written: pre-flight makes its turn without writing
/// them again first.
pub const FILES_READY: &str = "files_ready";

/// The source panel retains the precise failure and the explicit recovery action.
pub(crate) const SOURCE_FAILED: &str = "Source preparation failed. Choose Prepare again to retry.";

pub(crate) fn source_failed(thread: &ThreadRow) -> Result<bool, EngineError> {
    thread
        .source
        .as_ref()
        .map(|source| {
            serde_json::from_value::<SourceInfo>(source.get("info").unwrap_or(source).clone())
                .map(|info| info.state == SourceState::Failed)
        })
        .transpose()
        .map(|failed| failed.unwrap_or(false))
        .map_err(Into::into)
}

/// What the gate says of the input pre-flight would start a turn for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Nothing holds it back: the turn may be made.
    Pass,
    /// Its files are being written first: the thread is `starting` without a turn.
    Preparing,
}

/// What a preparation came to.
enum Prepared {
    /// Every file is where the agent reads it.
    Ready,
    /// Some could not be written: each, with why.
    Held(Json),
    /// The input goes back to waiting as it was (it is gone, or its environment is); pre-flight says why.
    Back,
    /// Its environment cannot be had now, for this reason: the input waits where it stood and the thread
    /// is blocked as `environment_unavailable`, as a turn's start leaves it.
    Unavailable(String),
    /// Its frozen source failed; only the explicit source retry may prepare it again.
    SourceFailed,
}

/// One `AttachmentIssue`.
fn issue(
    part: usize,
    collection: &str,
    index: Option<usize>,
    name: &str,
    code: &str,
    message: &str,
    retryable: bool,
) -> Json {
    json!({
        "part": part, "collection": collection, "index": index, "name": name,
        "code": code, "message": message, "retryable": retryable,
    })
}

/// Where a planned file is in its input: the part, the collection and the place in it.
fn place_of(slot: Slot) -> (usize, &'static str, Option<usize>) {
    match slot {
        Slot::File(part) | Slot::Image(part) => (part, "part", None),
        Slot::ForwardedImage(part, index) => (part, "images", Some(index)),
        Slot::ForwardedFile(part, index) => (part, "attachments", Some(index)),
    }
}

/// The issues of the files of an input that a delivery could not write, each with the reason it was given.
fn issues_of(input: Id, content: &[ContentPart], placement: &Placement) -> Vec<Json> {
    delivery::plan(input, content)
        .iter()
        .filter_map(|file| {
            placement.refusal_of(file.slot).map(|why| {
                let (part, collection, index) = place_of(file.slot);
                issue(
                    part,
                    collection,
                    index,
                    &file.name,
                    "delivery_failed",
                    why,
                    true,
                )
            })
        })
        .collect()
}

impl Engine {
    /// The gate pre-flight asks, in its transaction, once its checks have passed for `input`: whether the
    /// turn may be made now, or its files must be written first. Native media support is decided
    /// when constructing the model input, after every attachment has its verified workspace path.
    pub(crate) async fn files_gate(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        input: &mut InputRow,
    ) -> Result<Gate, EngineError> {
        let content: Vec<ContentPart> = serde_json::from_value(input.content.clone())?;
        let before = delivery::plan(input.id, &content);
        let content = self.prepare_pastes(unit.conn, thread, &content).await?;
        let after = delivery::plan(input.id, &content);
        if input.state_detail.as_deref() == Some(FILES_READY)
            && after.iter().any(|file| {
                !before
                    .iter()
                    .any(|old| old.path == file.path && old.blob == file.blob)
            })
        {
            input.state_detail = None;
        }
        let prepared = serde_json::to_value(&content)?;
        if prepared != input.content {
            sqlx::query("UPDATE thread_inputs SET content=$2 WHERE id=$1")
                .bind(input.id)
                .bind(&prepared)
                .execute(&mut *unit.conn)
                .await
                .map_err(orch_db::DbError::from)?;
            input.content = prepared;
        }
        if (delivery::plan(input.id, &content).is_empty()
            || input.state_detail.as_deref() == Some(FILES_READY))
            && crate::source::before_ready(unit, thread, input).await?
        {
            return Ok(Gate::Pass);
        }
        let seq = unit.stamp(thread.node_id).await?;
        threads::hold_input(&mut *unit.conn, input.id, Some(PREPARING), seq).await?;
        threads::set_state(
            &mut *unit.conn,
            thread.node_id,
            &[thread_state::IDLE, thread_state::BLOCKED],
            &StateChange {
                state: thread_state::STARTING,
                reason: None,
                detail: None,
                step: Some(if crate::source::ready(thread)? {
                    delivery::FILES_STEP
                } else {
                    "source"
                }),
            },
            seq,
        )
        .await?;
        unit.wake(thread.node_id);
        Ok(Gate::Preparing)
    }

    /// Holds an input for its files, with each issue, and says so in the audit: it keeps its text, parts,
    /// files and place until its author recovers it.
    async fn hold_for_files(
        &self,
        unit: &mut Unit<'_>,
        thread: Id,
        input: &InputRow,
        issues: Json,
    ) -> Result<(), EngineError> {
        let seq = unit.stamp(thread).await?;
        let now = unit.now();
        if threads::hold_for_attachments(&mut *unit.conn, input.id, &issues, seq).await?
            && let Some(author) = input.author_id
        {
            crate::audit_facts::person(
                &mut *unit.conn,
                author,
                thread,
                orch_db::audit::name!("input.held"),
                json!({ "input": input.id, "issues": issues.as_array().map_or(0, Vec::len) }),
                now,
            )
            .await?;
        }
        Ok(())
    }

    /// The supervisor's look at a thread that prepares an input: its environment started where it must be
    /// and waited for, the files written, and the input settled, after which pre-flight runs again. A
    /// thread left `starting` without a turn and with nothing to prepare (its input was cancelled) is put
    /// back as pre-flight finds it.
    pub(crate) async fn drive_preparation(&self, thread_id: Id) -> Result<(), EngineError> {
        let (thread, input, environment) = {
            let mut conn = self.inner.db.acquire().await?;
            let Some(thread) = threads::thread(&mut conn, thread_id).await? else {
                return Ok(());
            };
            if thread.state != thread_state::STARTING || thread.current_turn_id.is_some() {
                return Ok(());
            }
            let input = threads::pending_inputs(&mut conn, thread_id)
                .await?
                .into_iter()
                .find(|input| input.state_detail.as_deref() == Some(PREPARING));
            let environment = match thread.environment_id {
                Some(id) => environments::environment(&mut conn, id).await?,
                None => None,
            };
            (thread, input, environment)
        };
        let Some(input) = input else {
            return self.end_preparation(thread_id, None, Prepared::Back).await;
        };
        let Some(environment) = environment.filter(|environment| environment.deleted_at.is_none())
        else {
            return self
                .end_preparation(thread_id, Some(input.id), Prepared::Back)
                .await;
        };
        // The environment as the start of the turn will find it: one decision for both
        // (`Engine::environment_standing`), so that preparation never sends back an input that
        // pre-flight sends here again at once.
        match self.environment_standing(environment.id).await? {
            Standing::Ready => {
                self.set_step(
                    thread_id,
                    if crate::source::ready(&thread)? {
                        delivery::FILES_STEP
                    } else {
                        "source"
                    },
                )
                .await?;
            }
            Standing::Waiting => {
                self.set_step(thread_id, "environment").await?;
                return Ok(());
            }
            Standing::Unavailable(why) => {
                return self
                    .end_preparation(thread_id, Some(input.id), Prepared::Unavailable(why))
                    .await;
            }
        }
        // Source readiness precedes attachments and recording. A failed or pending declared
        // source retains this input without making a turn, a lease or a model request.
        if !crate::source::ready(&thread)? {
            if source_failed(&thread)? {
                return self
                    .end_preparation(thread_id, Some(input.id), Prepared::SourceFailed)
                    .await;
            }
            self.prepare_pending_source(&thread).await?;
            return Ok(());
        }
        let content: Vec<ContentPart> = serde_json::from_value(input.content.clone())?;
        let directory =
            crate::sessions::absolute_directory(&environment.workspace_root, &thread.workspace_dir);
        let Some(placement) = self
            .deliver_files(&Delivering {
                thread: thread_id,
                input: input.id,
                content: &content,
                environment: &environment,
                directory: &directory,
                sender: input.author_id,
            })
            .await?
        else {
            // The process is stopping: the next one finds the same work.
            return Ok(());
        };
        let issues = issues_of(input.id, &content, &placement);
        let prepared = if issues.is_empty() {
            self.record_before(&thread, &input, &environment).await?;
            Prepared::Ready
        } else {
            Prepared::Held(Json::Array(issues))
        };
        self.end_preparation(thread_id, Some(input.id), prepared)
            .await
    }

    /// Settles what was prepared, under the thread's lock, and runs pre-flight again in the same
    /// transaction: the thread is no longer `starting`, and pre-flight decides what it is now. An
    /// environment that cannot be had blocks the thread with its reason instead, as the start of a turn
    /// does (`fail_turn_start`): pre-flight asked again in this transaction would only send the input back
    /// here, and the thread is looked at again when the environment changes or at the next recheck.
    async fn end_preparation(
        &self,
        thread_id: Id,
        input: Option<Id>,
        prepared: Prepared,
    ) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            let Some(thread) = threads::lock(&mut *unit.conn, thread_id).await? else {
                return Ok(());
            };
            if thread.state != thread_state::STARTING || thread.current_turn_id.is_some() {
                return Ok(());
            }
            if matches!(prepared, Prepared::SourceFailed) && !source_failed(&thread)? {
                // A source retry or completion won the lock after this preparation read its failure.
                return Ok(());
            }
            let row = match input {
                Some(id) => threads::input(&mut *unit.conn, id).await?.filter(|row| {
                    row.state == input_state::PENDING
                        && row.state_detail.as_deref() == Some(PREPARING)
                }),
                None => None,
            };
            let blocked = match &prepared {
                Prepared::Unavailable(why) => Some(("environment_unavailable", why.clone())),
                Prepared::SourceFailed => Some(("error", SOURCE_FAILED.to_owned())),
                Prepared::Ready | Prepared::Held(_) | Prepared::Back => None,
            };
            if let Some(row) = &row {
                match prepared {
                    Prepared::Ready => {
                        let seq = unit.stamp(thread_id).await?;
                        threads::hold_input(&mut *unit.conn, row.id, Some(FILES_READY), seq)
                            .await?;
                    }
                    Prepared::Held(issues) => {
                        self.hold_for_files(&mut unit, thread_id, row, issues)
                            .await?;
                    }
                    Prepared::Back | Prepared::Unavailable(_) | Prepared::SourceFailed => {
                        let seq = unit.stamp(thread_id).await?;
                        threads::hold_input(&mut *unit.conn, row.id, None, seq).await?;
                    }
                }
            }
            let seq = unit.stamp(thread_id).await?;
            if let Some((reason, why)) = blocked {
                crate::notifications::set_state(
                    &mut unit,
                    thread_id,
                    &[thread_state::STARTING],
                    &StateChange::blocked(reason, Some(why.as_str())),
                    seq,
                )
                .await?;
            } else {
                threads::set_state(
                    &mut *unit.conn,
                    thread_id,
                    &[thread_state::STARTING],
                    &StateChange::plain(thread_state::IDLE),
                    seq,
                )
                .await?;
                self.preflight_in(&mut unit, thread_id).await?;
                unit.wake(thread_id);
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// A steer whose files could not all be written: held before anything is sent into the turn, out of that
    /// turn, each file named, for its author to recover. Says whether it was held (and so is not to be sent).
    pub(crate) async fn hold_steer_for_files(
        &self,
        thread_id: Id,
        input: &InputRow,
        content: &[ContentPart],
        placement: &Placement,
    ) -> Result<bool, EngineError> {
        let issues = issues_of(input.id, content, placement);
        if issues.is_empty() {
            return Ok(false);
        }
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            if threads::lock(&mut *unit.conn, thread_id).await?.is_none() {
                return Ok(true);
            }
            self.hold_for_files(&mut unit, thread_id, input, Json::Array(issues))
                .await?;
            unit.wake(thread_id);
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(true)
    }

    /// Tries again, for its author, the files that hold an input: the same input, released and looked at by
    /// pre-flight as any other, and held again if they still cannot go. A duplicate retry changes nothing.
    pub async fn retry_input(
        &self,
        caller: &orch_authz::Caller,
        thread_id: Id,
        input_id: Id,
    ) -> Result<orch_api_contract::threads::ThreadInput, crate::inputs::InputError> {
        let tx = self.inner.db.begin().await?;
        let now = self.inner.clock.now();
        self.retry_input_with(tx, caller, now, thread_id, input_id)
            .await
    }

    /// As [`Engine::retry_input`], in the transaction of the API's action guard, with its caller and its time.
    pub async fn retry_input_guarded(
        &self,
        tx: orch_db::Tx,
        principal: orch_authz::Principal,
        thread_id: Id,
        input_id: Id,
    ) -> Result<orch_api_contract::threads::ThreadInput, crate::inputs::InputError> {
        let caller = principal.caller().clone();
        let now = principal.now().get();
        self.retry_input_with(tx, &caller, now, thread_id, input_id)
            .await
    }

    async fn retry_input_with(
        &self,
        mut tx: orch_db::Tx,
        caller: &orch_authz::Caller,
        now: chrono::DateTime<chrono::Utc>,
        thread_id: Id,
        input_id: Id,
    ) -> Result<orch_api_contract::threads::ThreadInput, crate::inputs::InputError> {
        use crate::inputs::InputError;
        let (finished, view) = {
            let mut unit = Unit::new(&mut tx, now);
            let thread = threads::lock(&mut *unit.conn, thread_id)
                .await?
                .ok_or(InputError::NotFound)?;
            if thread.archived_at.is_some() {
                return Err(InputError::Archived);
            }
            // Whoever may drive the thread recovers their files; a message that waits for somebody who may use
            // the model keeps waiting for that afterwards.
            self.check_author(&mut unit, caller, &thread).await?;
            let input = threads::input(&mut *unit.conn, input_id)
                .await?
                .filter(|input| input.thread_id == thread_id)
                .ok_or(InputError::NotFound)?;
            if input.continuation_id != thread.active_continuation_id {
                return Err(InputError::InactiveContinuation {
                    continuation_id: input.continuation_id,
                    active_continuation_id: thread.active_continuation_id,
                });
            }
            // Recovery of an input is its author's; of one nobody sent, the thread's first owner's.
            if input.author_id.unwrap_or(thread.owner_id) != caller.user() {
                return Err(InputError::NotTheAuthor);
            }
            if input.state != input_state::PENDING
                || input.state_detail.as_deref() != Some(HELD_ATTACHMENTS)
            {
                return Err(InputError::NotHeld {
                    state: input.state.clone(),
                    detail: input.state_detail.clone(),
                });
            }
            let seq = unit.stamp(thread_id).await?;
            threads::release_attachment_hold(&mut *unit.conn, input_id, seq).await?;
            crate::audit_facts::person(
                &mut *unit.conn,
                caller.user(),
                thread_id,
                orch_db::audit::name!("input.recovered"),
                json!({ "input": input_id }),
                now,
            )
            .await?;
            if matches!(
                thread.state.as_str(),
                thread_state::IDLE | thread_state::BLOCKED
            ) {
                self.preflight_in(&mut unit, thread_id).await?;
            }
            unit.wake(thread_id);
            let latest = threads::input(&mut *unit.conn, input_id)
                .await?
                .ok_or(InputError::NotFound)?;
            let people = crate::views::People::load(
                &mut *unit.conn,
                latest.author_id.into_iter().chain(latest.run_as),
            )
            .await?;
            let view = crate::views::input_view(&latest, &people)?;
            (unit.finish().await?, view)
        };
        self.commit(tx, finished).await?;
        Ok(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_are_planned_for_file_delivery_even_without_native_image_input() {
        let content = vec![ContentPart::Image {
            blob: orch_types::Hash::of(b"png"),
            media_type: orch_api_contract::threads::ImageMediaType::Png,
            name: "shot.png".into(),
            path: None,
        }];
        let planned = delivery::plan(orch_types::new_id(), &content);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].slot, Slot::Image(0));
        assert!(planned[0].path.ends_with("/1-shot.png"));
        let placement = Placement::refused(&planned, "the machine takes no files");
        let issues = issues_of(orch_types::new_id(), &content, &placement);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0]["code"], "delivery_failed");
        assert_eq!(issues[0]["name"], "shot.png");
    }
}
