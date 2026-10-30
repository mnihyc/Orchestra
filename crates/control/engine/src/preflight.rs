//! Atomic checks before a turn: each act against whoever does it (the author of an input; the thread's first owner for an input no person sent, which waits without holding the others up), availability, model, effort and confinement; the limits an input was let past left out.
//!
//! Pre-flight runs in one transaction, with the thread's row locked,
//! whenever a thread might start a turn: an input arrived on an idle thread,
//! a turn ended with inputs pending, a blocked thread's cause may have
//! cleared. The checks run in the order of the design and the first failure
//! sets the thread `blocked` with its reason and stops. On success the same
//! transaction opens the lease, makes the turn and its segment, sets the
//! input to `delivering` and the thread to `starting`; the commands follow
//! after the commit and are repeated by nobody, since their identifiers are
//! derived from what they are about.
//!
//! **A cause of the thread's own stays in view.** A thread blocked because of what it is bound to or where it
//! runs (no provider, a profile switched off, an environment gone, a model not set) stays blocked for that cause
//! when its queue empties (the input was cancelled, say), and becomes idle once, when the cause is mended: the
//! checks 2 to 11 are asked again without an input. A block that is about the work that waits (a cap reached, a
//! start that failed) ends with that work.
//!
//! **Nothing is let through that was not checked.** A question that cannot
//! be answered (the resource side of the access engine is not connected, a
//! setting does not read) is an error of the whole pre-flight: nothing is
//! started and nothing is cancelled, and the thread is looked at again.
//!
//! The provider's side has a check of its own where the turn goes through the
//! forwarder: the machine that keeps the key must be there, online and in the
//! role of a provider, else the thread waits as `provider_unavailable`; a
//! provider whose machine is away has stood `offline` since the supervisor saw
//! it go, which check 5 refuses the same way, and it is `ready` again, and the
//! thread looked at again, when the machine returns.
//!
//! **The environment** is available when its status is `running` and its machine is online.
//! One made from a template that is `none`, `stopped` or being made is let through: the start
//! sequence of the turn starts it and the thread waits in `starting`, saying so
//! (`Engine::await_environment`); so is a directory on a person's machine that is not attached yet
//! while its machine has a session, since the schedule attaches it and the sequence waits for that.
//! One that failed, is offline, is being replaced or moved, or whose removal is scheduled blocks the
//! thread as `environment_unavailable` with the reason.
//!
//! **What the checks assume of parts not built:** a credential path on which a credential leaves
//! the provider (`shared`, `handover`) blocks as `binding_invalid`.
//!
//! **The caps come last (check 12).** Once the binding holds, the caps that
//! cover the turn are asked (see `caps`): one without room that is shared blocks
//! the thread as `quota_exceeded` and says which; one counted per person holds
//! the input of that person, which keeps its place while pre-flight goes on with
//! the next; a limit the input was let past by somebody who still holds the right
//! is left out. Held inputs are looked at again each time and released when room
//! is made, in their order.

use crate::access::{Answer, Limit, ProviderUse};
use crate::binding::{self, Facts};
use crate::caps::{self, Ask, CapCheck, Verdict};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::harnesses;
use crate::inputs::{InputAuthority, authority_caller, input_authority, saved_authority};
use crate::leases;
use crate::resolved::{self, Resolved};
use crate::sessions;
use crate::unit::Unit;
use orch_access_contract::decisions::Decision;
use orch_access_contract::permissions::Permission;
use orch_agent_contract::arrangement::{Arrangement, Metering, metering};
use orch_agent_contract::credentials::CredentialPath;
use orch_agent_contract::descriptor::{AgentDescriptor, ArrangementType};
use orch_agent_contract::parameters::ModelParameters;
use orch_authz::{Act, Caller, Principal, nodes};
use orch_db::queries::environments::thread_engine::{self as environments, EnvironmentRow};
use orch_db::queries::resources::thread_engine::{self as resources, AgentRow, ProviderRow};
use orch_db::queries::threads::{
    self, HELD_ATTACHMENTS, HELD_CAP_REACHED, HELD_MODEL_USE, HELD_OWNER_LACKS_ACCESS,
    HELD_SECOND_STEP, InputChange, InputRow, NewTurn, StateChange, ThreadRow, input_state,
    thread_state,
};
use orch_settings::{Place, Snapshot, Wanted, get as setting};
use orch_settings_contract::registry::{self as setting_defs, keys as setting_keys};
use orch_types::{Id, Json, Time, new_id};

/// The value of `auth.second_step` that requires a second step of the people of a team org.
const SECOND_STEP_REQUIRED: &str = "required";

/// The `state_detail` of an input cancelled because its author may no longer drive the thread. An author who
/// may drive it but not use its model or its environment is never cancelled for that (`thread.inputs_without_use`).
pub const AUTHOR_LOST_ACCESS: &str = "author_lost_access";

/// `thread.inputs_without_use`: such an input waits for somebody who may use the model and the environment (the
/// default).
pub const WITHOUT_MODEL_HELD: &str = "held";
/// `thread.inputs_without_use`: such an input is refused when it is sent; one already waiting is held.
pub const WITHOUT_MODEL_REFUSED: &str = "refused";
/// `thread.inputs_without_use`: such an input runs on the access of the thread's first owner.
pub const WITHOUT_MODEL_ALLOWED: &str = "allowed";

/// The reasons of a block whose cause is the thread itself (what it is bound to, and where it runs), and not an
/// input that waits: a thread blocked for one of them keeps it while its queue is empty, until the cause is mended.
/// A reached cap (`quota_exceeded`), a failed start (`error`) and a switch (`switching`) are about the work that
/// waits or the job that holds it, and are not among them.
pub(crate) const CAUSES_OF_THE_THREAD: &[&str] = &[
    "unbound",
    "agent_offline",
    "agent_disabled",
    "provider_unavailable",
    "provider_exhausted",
    "environment_unavailable",
    "binding_invalid",
    "provider_not_allowed",
    "model_not_set",
    "model_not_allowed",
    crate::trust::WAITING,
];

/// Why a thread cannot start a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    /// The reason, in the words the thread's column has.
    pub reason: &'static str,
    /// A sentence about this occurrence.
    pub detail: Option<String>,
}

impl Blocked {
    /// An explained block uses the thread's existing reason vocabulary.
    pub(crate) fn new(reason: &'static str, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: Some(detail.into()),
        }
    }
}

/// A turn that was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Started {
    /// The thread.
    pub thread: Id,
    /// The turn.
    pub turn: Id,
    /// The input it answers.
    pub input: Id,
    /// The segment it is in.
    pub segment: Id,
}

/// What a pre-flight came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// There is no such thread.
    Gone,
    /// The thread is archived or deleted: nothing runs in it.
    Dormant,
    /// A turn is in flight; it is the one that is waited for.
    Busy,
    /// Nothing waits: the thread is idle.
    Idle,
    /// Something waits and cannot start.
    Blocked(Blocked),
    /// A turn was made.
    Started(Started),
    /// The input's files are written first: the thread is `starting` without a turn.
    Preparing {
        /// The thread.
        thread: Id,
        /// The input whose files are written.
        input: Id,
    },
}

/// What became of the person an input is asked of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Person {
    Fine,
    Lost,
    Held,
    /// They may drive the thread but may not use its provider with its model and effort, or the environment it
    /// runs in: their input is served as the place's `thread.inputs_without_use` says, never cancelled for this.
    NeedsModel,
    Dormant,
    /// The team org the thread lies in requires a second step of them, and they have none set up: their
    /// input waits in its place (`second_step_required`), never cancelled for this, and the next goes ahead.
    Setup,
}

/// Everything a turn is set up from, once the checks have passed.
struct Plan {
    agent: AgentRow,
    provider: ProviderRow,
    environment: EnvironmentRow,
    model: String,
    effort: Option<String>,
    params: Json,
    /// The service tier the turn asks for, among its parameters; none is the ordinary tier.
    tier: Option<String>,
    arrangement: Arrangement,
    path: CredentialPath,
    /// Who reports what the turn uses.
    metering: Metering,
    /// Who let the input past the provider's own limit, where it is reached.
    provider_limit_by: Option<Id>,
    /// What the lease carries of the room the caps leave.
    allowance: Option<Json>,
    /// The limits the input is let past, and by whom.
    passed: Vec<caps::Passed>,
    /// The bypass of the input as it stands after the entries that no longer hold are dropped.
    bypass: Option<Json>,
}

fn parse_arrangement(word: Option<&str>) -> Option<ArrangementType> {
    word.and_then(|word| serde_json::from_value(Json::String(word.to_owned())).ok())
}

impl Engine {
    /// Runs pre-flight for a thread in a transaction of its own, and sees
    /// that what follows a started turn is taken up.
    pub async fn preflight(&self, thread: Id) -> Result<Outcome, EngineError> {
        let selected: bool = {
            let mut conn = self.db().acquire().await?;
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_inputs WHERE thread_id=$1 AND state='pending' AND native_action IS NOT NULL)")
                .bind(thread).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?
        };
        if selected {
            self.native_actions(thread, true).await?;
        }
        let mut tx = self.inner.db.begin().await?;
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        let outcome = self.preflight_in(&mut unit, thread).await?;
        let finished = unit.finish().await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        for woken in finished.wakes {
            self.wake(woken);
        }
        Ok(outcome)
    }

    /// Pre-flight inside the transaction of a caller, which commits.
    pub(crate) async fn preflight_in(
        &self,
        unit: &mut Unit<'_>,
        thread_id: Id,
    ) -> Result<Outcome, EngineError> {
        let Some(thread) = threads::lock(&mut *unit.conn, thread_id).await? else {
            return Ok(Outcome::Gone);
        };
        if thread.deleted_at.is_some() || thread.archived_at.is_some() {
            return Ok(Outcome::Dormant);
        }
        if orch_db::queries::agent_history::removing(unit.conn, thread_id).await? {
            let blocked = Blocked::new(
                "error",
                "Its agent history is being removed. The next session carries the conversation in.",
            );
            self.block(unit, &thread, &blocked).await?;
            return Ok(Outcome::Blocked(blocked));
        }
        // An ended turn's retained accounting is part of admission, including direct input
        // requests which can arrive before its ordinary supervisor wake is processed.
        if threads::turn_end_work_pending(&mut *unit.conn, thread_id).await? {
            unit.wake(thread_id);
            if thread.state == thread_state::BLOCKED
                && thread.blocked_reason.as_deref() == Some("error")
                && thread.blocked_detail.as_deref() == Some(crate::turns::END_FOLLOWUP_FAILED)
            {
                return Ok(Outcome::Blocked(Blocked::new(
                    "error",
                    crate::turns::END_FOLLOWUP_FAILED,
                )));
            }
            // A thread blocked by an error waits for somebody to retry or for another input, and this is one of
            // them: it is not lost while the end of the last turn is still being written. The thread is idle
            // again with its input waiting, and the pre-flight after that end decides.
            if thread.state == thread_state::BLOCKED
                && thread.blocked_reason.as_deref() == Some("error")
                && thread.current_turn_id.is_none()
                && threads::count_pending_inputs(&mut *unit.conn, thread_id).await? > 0
            {
                self.settle(unit, &thread, StateChange::plain(thread_state::IDLE))
                    .await?;
            }
            return Ok(Outcome::Busy);
        }
        // A steer sent to the previous turn may have been taken even when its
        // acknowledgement has not arrived. Its result decides whether it is
        // delivered or goes back to the queue before another turn starts.
        if thread.current_turn_id.is_none()
            && threads::unsettled_steer(&mut *unit.conn, thread_id).await?
        {
            unit.wake(thread_id);
            return Ok(Outcome::Busy);
        }
        if crate::calls::maybe_return_in(unit, &thread).await? {
            return Ok(Outcome::Busy);
        }
        if let Some(recovery) = crate::calls::read(
            unit.conn,
            thread_id,
            orch_provider_contract::calls::Purpose::Agent,
        )
        .await?
            && matches!(
                recovery.state.as_str(),
                "scheduled" | "switching" | "waiting" | "stopped"
            )
        {
            return Ok(Outcome::Blocked(Blocked::new("error", "call_recovery")));
        }
        if orch_db::queries::lifecycle::closing(&mut *unit.conn, thread_id).await? {
            let blocked = Blocked::new("error", "Its previous session is being closed.");
            self.block(unit, &thread, &blocked).await?;
            return Ok(Outcome::Blocked(blocked));
        }
        if thread.environment_id.is_none() {
            let blocked = Blocked::new("unbound", "Choose an environment before starting work.");
            self.block(unit, &thread, &blocked).await?;
            return Ok(Outcome::Blocked(blocked));
        }
        if !matches!(
            thread.state.as_str(),
            thread_state::IDLE | thread_state::BLOCKED
        ) || threads::active_turn(&mut *unit.conn, thread_id)
            .await?
            .is_some()
        {
            return Ok(Outcome::Busy);
        }
        // A forced Stop preserves earlier retained commands and waits for actual native idle.
        if crate::background::native_busy(&mut *unit.conn, thread_id).await? {
            let blocked = Blocked {
                reason: "error",
                detail: Some(crate::background::NATIVE_BUSY.to_owned()),
            };
            self.block(unit, &thread, &blocked).await?;
            return Ok(Outcome::Blocked(blocked));
        }

        // The part that forks and switches: a thread that a switch holds, or that a fork is still making,
        // starts no turn and takes what is sent to it into its queue until the job lets go.
        if let Some(blocked) = crate::locks::blocked_while_held(&mut *unit.conn, thread_id).await? {
            self.block(unit, &thread, &blocked).await?;
            return Ok(Outcome::Blocked(blocked));
        }
        // A failed full-history preparation cannot turn a retained target's
        // queued authored words into a fresh, empty native conversation. Browsing
        // that target is passive; only the retained preparation can make it ready.
        let unready: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM thread_native_guards
            WHERE thread_id=$1 AND NOT ready)",
        )
        .bind(thread_id)
        .fetch_one(&mut *unit.conn)
        .await
        .map_err(orch_db::DbError::from)?;
        if unready {
            let blocked = Blocked::new(
                "error",
                "Its confirmed history is not ready. Retry the retained preparation before running queued words.",
            );
            self.block(unit, &thread, &blocked).await?;
            return Ok(Outcome::Blocked(blocked));
        }
        if thread.side_of.is_some()
            && let Some(segment) = thread.current_segment_id
            && threads::segment(unit.conn, segment)
                .await?
                .is_some_and(|segment| segment.native_session_ref.is_none())
        {
            let at = unit.now();
            match crate::side_threads::history::authorize(self, unit.conn, &thread, at).await {
                Ok(_) => {}
                Err(EngineError::Unreadable(detail)) => {
                    let blocked = Blocked::new("error", detail);
                    self.block(unit, &thread, &blocked).await?;
                    return Ok(Outcome::Blocked(blocked));
                }
                Err(error) => return Err(error),
            }
        }
        let settings = resolved::load(&mut *unit.conn, thread_id).await?;
        if let Some(blocked) = crate::network::blocked_thread(&mut *unit.conn, thread_id).await? {
            self.block(unit, &thread, &blocked).await?;
            return Ok(Outcome::Blocked(blocked));
        }

        // Everybody who waits is asked whether they may still be served: a person who may no longer drive the
        // thread is cancelled; an input without an author is held or released by what its owner may do; a writer
        // who may not use the thread's model is served as the place says, and never cancelled for it.
        let without_model = Box::pin(setting::<String>(
            &mut *unit.conn,
            Place::Node(thread_id),
            setting_keys::THREAD_INPUTS_WITHOUT_USE,
        ))
        .await?
        .unwrap_or_else(|| WITHOUT_MODEL_HELD.to_owned());
        let mut verdicts: Vec<(Option<Id>, Option<InputAuthority>, Person)> = Vec::new();
        for input in threads::pending_inputs(&mut *unit.conn, thread_id).await? {
            let person = if input.author_id.is_none() && !input.is_authorless() {
                Person::Lost
            } else {
                Box::pin(self.person_of(
                    unit,
                    &thread,
                    input.author_id,
                    input.submission_authority.as_ref(),
                    &settings,
                    &mut verdicts,
                ))
                .await?
            };
            match person {
                Person::Dormant => return Ok(Outcome::Dormant),
                Person::Fine => {
                    // The author may serve themself: they answer for it again, and what it waited for is over.
                    if input.run_as.is_some() {
                        let seq = unit.stamp(thread_id).await?;
                        threads::set_input_run_as(&mut *unit.conn, input.id, None, None, seq)
                            .await?;
                    }
                    if matches!(
                        input.state_detail.as_deref(),
                        Some(HELD_OWNER_LACKS_ACCESS | HELD_SECOND_STEP | HELD_MODEL_USE)
                    ) {
                        let seq = unit.stamp(thread_id).await?;
                        threads::hold_input(&mut *unit.conn, input.id, None, seq).await?;
                        if input.state_detail.as_deref() == Some(HELD_MODEL_USE) {
                            self.cancel_source_request(
                                unit,
                                orch_db::queries::notifications::inbox::RequestSource::Input(
                                    input.id,
                                ),
                                None,
                            )
                            .await?;
                        }
                    }
                }
                Person::Setup => {
                    if input.state_detail.as_deref() != Some(HELD_SECOND_STEP) {
                        let seq = unit.stamp(thread_id).await?;
                        threads::hold_input(&mut *unit.conn, input.id, Some(HELD_SECOND_STEP), seq)
                            .await?;
                    }
                }
                Person::Held => {
                    if input.state_detail.as_deref() != Some(HELD_OWNER_LACKS_ACCESS) {
                        let seq = unit.stamp(thread_id).await?;
                        threads::hold_input(
                            &mut *unit.conn,
                            input.id,
                            Some(HELD_OWNER_LACKS_ACCESS),
                            seq,
                        )
                        .await?;
                    }
                }
                Person::NeedsModel => {
                    Box::pin(self.serve_without_model(
                        unit,
                        &thread,
                        &input,
                        &without_model,
                        &settings,
                        &mut verdicts,
                    ))
                    .await?;
                }
                Person::Lost => {
                    let seq = unit.stamp(thread_id).await?;
                    threads::change_input(
                        &mut *unit.conn,
                        input.id,
                        &[input_state::PENDING],
                        &InputChange {
                            state: input_state::CANCELLED,
                            detail: Some(AUTHOR_LOST_ACCESS),
                            ..InputChange::default()
                        },
                        seq,
                    )
                    .await?;
                }
            }
        }

        let mut pending = threads::pending_inputs(&mut *unit.conn, thread_id).await?;
        // Inputs held for their author's own cap are looked at again: room made, or somebody who holds the
        // row letting the input past, releases them in their place.
        if pending
            .iter()
            .any(|input| input.state_detail.as_deref() == Some(HELD_CAP_REACHED))
            && let Some(ask) = self.ask_of_thread(unit, &thread, &settings).await?
            && self.release_held_inputs(unit, &ask, &pending).await?
        {
            pending = threads::pending_inputs(&mut *unit.conn, thread_id).await?;
        }
        let mut held_for: Option<String> = None;
        loop {
            let Some(mut input) = pending.iter().find(|input| !input.is_held()).cloned() else {
                // What waits for a person (what a Stop held, files that cannot reach the agent) holds nobody
                // else up and is not the thread's block; what waits for room in a cap, or for the owner, is.
                let waiting: Vec<&InputRow> = pending
                    .iter()
                    .filter(|input| !input.waits_for_a_person())
                    .collect();
                if waiting.is_empty() {
                    // A cause that is the thread's own stays in view though nothing waits any more: the thread
                    // is idle once, when the cause is mended, and not because its queue emptied.
                    if let Some(blocked) =
                        self.cause_of_the_thread(unit, &thread, &settings).await?
                    {
                        self.block(unit, &thread, &blocked).await?;
                        return Ok(Outcome::Blocked(blocked));
                    }
                    // Only inputs whose files cannot reach the agent wait: the thread says so, without a turn,
                    // until each author recovers theirs. What a Stop held is the queue's, and the thread is idle.
                    let for_files = pending.iter().filter(|input| {
                        !input.queue_held && input.state_detail.as_deref() == Some(HELD_ATTACHMENTS)
                    });
                    if let Some(count) = std::num::NonZeroUsize::new(for_files.count()) {
                        let blocked = Blocked::new(
                            "attachments",
                            format!(
                                "{count} message(s) wait for files that cannot reach the agent"
                            ),
                        );
                        self.block(unit, &thread, &blocked).await?;
                        return Ok(Outcome::Blocked(blocked));
                    }
                    if pending.iter().any(|input| {
                        input
                            .state_detail
                            .as_deref()
                            .is_some_and(|detail| detail.starts_with("native_action."))
                    }) {
                        let blocked=Blocked::new("error","A selected command changed or is unavailable; refresh or remove that selection".to_owned());
                        self.block(unit, &thread, &blocked).await?;
                        return Ok(Outcome::Blocked(blocked));
                    }
                    self.settle(unit, &thread, StateChange::plain(thread_state::IDLE))
                        .await?;
                    return Ok(Outcome::Idle);
                }
                let blocked = if waiting
                    .iter()
                    .all(|input| input.state_detail.as_deref() == Some(HELD_SECOND_STEP))
                {
                    // Only work whose authors' second step is missing waits: their setup is the remedy.
                    Blocked::new("binding_invalid", HELD_SECOND_STEP)
                } else if waiting.iter().all(|input| {
                    matches!(
                        input.state_detail.as_deref(),
                        Some(HELD_OWNER_LACKS_ACCESS | HELD_SECOND_STEP)
                    )
                }) {
                    Blocked::new("binding_invalid", HELD_OWNER_LACKS_ACCESS)
                } else {
                    Blocked::new(
                        "quota_exceeded",
                        held_for
                            .clone()
                            .unwrap_or_else(|| HELD_CAP_REACHED.to_owned()),
                    )
                };
                self.block(unit, &thread, &blocked).await?;
                return Ok(Outcome::Blocked(blocked));
            };

            if crate::native_actions::hold_invalid(unit, thread_id, &input).await? {
                if let Some(row) = pending.iter_mut().find(|row| row.id == input.id) {
                    row.state_detail = Some("native_action.unavailable".to_owned());
                }
                continue;
            }
            let mut plan = match self
                .check_binding(unit, &thread, Some(&input), &settings)
                .await?
            {
                Err(blocked) => {
                    self.block(unit, &thread, &blocked).await?;
                    return Ok(Outcome::Blocked(blocked));
                }
                Ok(plan) => plan,
            };
            // A terminal source failure keeps the same input without re-entering preparation on every wake.
            if crate::preparation::source_failed(&thread)? {
                let blocked = Blocked::new("error", crate::preparation::SOURCE_FAILED);
                self.block(unit, &thread, &blocked).await?;
                return Ok(Outcome::Blocked(blocked));
            }
            // 12
            let ask = Ask {
                thread: thread_id,
                path: nodes_path(&mut *unit.conn, thread_id).await?,
                provider: plan.provider.id,
                model: plan.model.clone(),
                tier: plan.tier.clone(),
                harness_kind: Some(plan.agent.harness_kind.clone()),
                person: input.answers(),
                metering: Some(plan.metering),
            };
            let check = self.check_caps(unit, &input, &ask).await?;
            match &check.verdict {
                Verdict::Clear => {
                    // Files are written before anything opens for the turn, with the thread starting and no turn yet.
                    match self.files_gate(unit, &thread, &mut input).await? {
                        crate::preparation::Gate::Pass => {}
                        crate::preparation::Gate::Preparing => {
                            return Ok(Outcome::Preparing {
                                thread: thread_id,
                                input: input.id,
                            });
                        }
                    }
                    plan.allowance = self
                        .allowance_of(&mut *unit.conn, &check.room, &ask)
                        .await?;
                    plan.bypass = check.bypass.clone();
                    plan.passed = check.passed.clone();
                    // The login is taken for the thread's environment in the transaction that starts the
                    // turn, in one compare-and-set; a thread that lost the race waits as the others do.
                    if plan.path == CredentialPath::Handover
                        && plan.provider.held_by_environment_id.is_none()
                        && !self
                            .take_credential(
                                unit,
                                &plan.provider,
                                &plan.environment,
                                thread_id,
                                input.answers(),
                            )
                            .await?
                    {
                        // Somebody took it between the look and the compare-and-set: if that was a thread
                        // of this very environment, the credential is where this turn needs it.
                        let holder = orch_db::queries::resources::provider_runtime::state(
                            &mut *unit.conn,
                            plan.provider.id,
                        )
                        .await?
                        .and_then(|state| state.held_by_environment_id);
                        if holder != Some(plan.environment.id) {
                            let blocked = Blocked::new("provider_unavailable", "handed_over");
                            self.block(unit, &thread, &blocked).await?;
                            return Ok(Outcome::Blocked(blocked));
                        }
                    }
                    return Ok(Outcome::Started(
                        self.make_turn(unit, &thread, &input, plan).await?,
                    ));
                }
                Verdict::Blocked(detail) => {
                    if let Some(cap) = check.blocked_by {
                        self.note_waiting_for(thread_id, cap);
                    }
                    self.forget_stale_bypass(unit, &thread, &input, &check)
                        .await?;
                    let blocked = Blocked::new("quota_exceeded", detail.clone());
                    self.block(unit, &thread, &blocked).await?;
                    return Ok(Outcome::Blocked(blocked));
                }
                Verdict::Held(detail) => {
                    // The author's own allowance is used up: their input waits in its place, and the next goes ahead.
                    self.forget_stale_bypass(unit, &thread, &input, &check)
                        .await?;
                    let seq = unit.stamp(thread_id).await?;
                    threads::hold_input(&mut *unit.conn, input.id, Some(HELD_CAP_REACHED), seq)
                        .await?;
                    held_for = Some(detail.clone());
                    pending = threads::pending_inputs(&mut *unit.conn, thread_id).await?;
                }
            }
        }
    }

    /// Takes out of an input that waits the entries that no longer hold, so that the page does not keep a pass
    /// that nobody can give any more.
    async fn forget_stale_bypass(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        input: &InputRow,
        check: &CapCheck,
    ) -> Result<(), EngineError> {
        if check.bypass_changed {
            let seq = unit.stamp(thread.node_id).await?;
            threads::set_input_bypass(
                &mut *unit.conn,
                input.id,
                &check
                    .bypass
                    .clone()
                    .unwrap_or_else(|| Json::Array(Vec::new())),
                seq,
            )
            .await?;
        }
        Ok(())
    }

    /// Sets the thread's state, where it is not that already.
    async fn settle(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        to: StateChange<'_>,
    ) -> Result<(), EngineError> {
        let same = thread.state == to.state
            && thread.blocked_reason.as_deref() == to.reason
            && thread.blocked_detail.as_deref() == to.detail
            && thread.starting_step.as_deref() == to.step;
        if same {
            return Ok(());
        }
        let seq = unit.stamp(thread.node_id).await?;
        crate::notifications::set_state(
            unit,
            thread.node_id,
            &[
                thread_state::IDLE,
                thread_state::STARTING,
                thread_state::BLOCKED,
            ],
            &to,
            seq,
        )
        .await?;
        Ok(())
    }

    /// Blocks the thread for a reason, unless it is blocked so already.
    async fn block(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        blocked: &Blocked,
    ) -> Result<(), EngineError> {
        self.settle(
            unit,
            thread,
            StateChange::blocked(blocked.reason, blocked.detail.as_deref()),
        )
        .await
    }

    /// For a thread that is blocked by a cause of its own and has nothing waiting: whether the cause is still
    /// there, asked as checks 2 to 11 ask it without an input. The answer is the cause as it stands now (it may be
    /// another of the thread's own); none when the thread could start a turn, or was not blocked so.
    async fn cause_of_the_thread(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        settings: &Resolved,
    ) -> Result<Option<Blocked>, EngineError> {
        if thread.state != thread_state::BLOCKED {
            return Ok(None);
        }
        let Some(reason) = thread.blocked_reason.as_deref() else {
            return Ok(None);
        };
        // Inputs held for their owner, or for their authors' second step, were the cause, and they are gone
        // with the queue.
        let held_for_owner = reason == "binding_invalid"
            && matches!(
                thread.blocked_detail.as_deref(),
                Some(HELD_OWNER_LACKS_ACCESS | HELD_SECOND_STEP)
            );
        if !CAUSES_OF_THE_THREAD.contains(&reason) || held_for_owner {
            return Ok(None);
        }
        Ok(self
            .check_binding(unit, thread, None, settings)
            .await?
            .err())
    }

    /// Check 1: may the person an input is asked of still be served?
    async fn check_person(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        author: Option<Id>,
        authority: Option<&InputAuthority>,
        settings: &Resolved,
    ) -> Result<Person, EngineError> {
        let now = Time::new(unit.now());
        let ask = |person: Id| ProviderUse {
            person,
            node: thread.node_id,
            provider: thread.provider_id.unwrap_or_else(Id::nil),
            model: settings.model.as_deref(),
            effort: settings.effort.as_deref(),
        };
        let Some(author) = author else {
            // An input nobody sent runs on what the thread's first owner may
            // use, and waits while that fails. Nobody else's access is asked.
            if thread.provider_id.is_none() {
                return Ok(Person::Fine);
            }
            let answer = self
                .inner
                .access
                .provider_use(&mut *unit.conn, &ask(thread.owner_id))
                .await?;
            return Ok(if answer.is_allowed() {
                Person::Fine
            } else {
                Person::Held
            });
        };
        let caller = match authority {
            Some(authority) => {
                match authority_caller(unit.conn, author, thread.node_id, now, authority).await? {
                    Some(caller) => caller,
                    None => return Ok(Person::Lost),
                }
            }
            // Accepted older work may still be attributed, but its lost credential provenance cannot grant model use.
            None => Caller::session(author).with_admin_mode(false),
        };
        let principal = match Principal::load(&mut *unit.conn, caller.clone(), now).await {
            Ok(principal) => principal,
            Err(orch_authz::Error::NoSuchUser(_)) => return Ok(Person::Lost),
            Err(error) => return Err(error.into()),
        };
        let decision = nodes::check(
            &mut *unit.conn,
            &principal,
            thread.node_id,
            Permission::ThreadInteract,
            Act::Use,
            orch_authz::Policy::default(),
        )
        .await;
        match decision {
            Ok(Decision::Allowed { .. }) => {}
            Ok(Decision::Refused(_)) | Err(orch_authz::Error::Suspended { .. }) => {
                return Ok(Person::Lost);
            }
            Err(orch_authz::Error::Archived { .. }) => return Ok(Person::Dormant),
            Err(error) => return Err(error.into()),
        }
        // The team org the thread lies in may require a second step of the author: while none is set up their
        // work there waits, as a request of theirs there would; nothing is taken from them for it.
        if second_step_missing(&mut *unit.conn, author, thread.node_id).await? {
            return Ok(Person::Setup);
        }
        if authority.is_none()
            || !caller.permits(Permission::ProviderUse)
            || (thread.environment_id.is_some() && !caller.permits(Permission::EnvironmentUse))
        {
            return Ok(Person::NeedsModel);
        }
        if thread.provider_id.is_none() {
            return Ok(Person::Fine);
        }
        let answer = self
            .inner
            .access
            .provider_use(&mut *unit.conn, &ask(author))
            .await?;
        Ok(if answer.is_allowed() {
            Person::Fine
        } else {
            Person::NeedsModel
        })
    }

    /// What a person is to the thread now, asked once per pre-flight.
    async fn person_of(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        person: Option<Id>,
        authority: Option<&Json>,
        settings: &Resolved,
        verdicts: &mut Vec<(Option<Id>, Option<InputAuthority>, Person)>,
    ) -> Result<Person, EngineError> {
        let authority = saved_authority(authority)?;
        if let Some((_, _, known)) = verdicts
            .iter()
            .find(|(asked, saved, _)| *asked == person && *saved == authority)
        {
            return Ok(*known);
        }
        let verdict = self
            .check_person(unit, thread, person, authority.as_ref(), settings)
            .await?;
        verdicts.push((person, authority, verdict));
        Ok(verdict)
    }

    /// Serves an input whose author may drive the thread but may not use its model. Somebody who ran it on their
    /// own access answers for it while they may; where the place allows it, the thread's first owner does, as for
    /// an input without an author; otherwise it waits, kept in its place, for somebody who may use the model.
    async fn serve_without_model(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        input: &InputRow,
        policy: &str,
        settings: &Resolved,
        verdicts: &mut Vec<(Option<Id>, Option<InputAuthority>, Person)>,
    ) -> Result<(), EngineError> {
        let thread_id = thread.node_id;
        let owner_authority =
            input_authority(&Caller::session(thread.owner_id).with_admin_mode(false))?;
        let runner = match input.run_as {
            Some(person) => Some((person, input.run_as_authority.as_ref())),
            None if policy == WITHOUT_MODEL_ALLOWED => {
                Some((thread.owner_id, Some(&owner_authority)))
            }
            None => None,
        };
        if let Some((runner, authority)) = runner {
            if Box::pin(self.person_of(unit, thread, Some(runner), authority, settings, verdicts))
                .await?
                == Person::Fine
            {
                if input.run_as != Some(runner)
                    || input.run_as_authority.as_ref() != authority
                    || matches!(
                        input.state_detail.as_deref(),
                        Some(HELD_MODEL_USE | HELD_OWNER_LACKS_ACCESS)
                    )
                {
                    let seq = unit.stamp(thread_id).await?;
                    threads::set_input_run_as(
                        &mut *unit.conn,
                        input.id,
                        Some(runner),
                        authority,
                        seq,
                    )
                    .await?;
                    if input.state_detail.as_deref() == Some(HELD_OWNER_LACKS_ACCESS) {
                        threads::hold_input(&mut *unit.conn, input.id, None, seq).await?;
                    }
                }
                return Ok(());
            }
            if input.run_as.is_none() {
                // The owner's access was to serve it and the owner may not use the model either: it waits for
                // them, as an input without an author does.
                if input.state_detail.as_deref() != Some(HELD_OWNER_LACKS_ACCESS) {
                    let seq = unit.stamp(thread_id).await?;
                    threads::hold_input(
                        &mut *unit.conn,
                        input.id,
                        Some(HELD_OWNER_LACKS_ACCESS),
                        seq,
                    )
                    .await?;
                }
                return Ok(());
            }
            // Whoever ran it may no longer: it waits again for somebody who may.
            let seq = unit.stamp(thread_id).await?;
            threads::set_input_run_as(&mut *unit.conn, input.id, None, None, seq).await?;
        }
        if input.state_detail.as_deref() != Some(HELD_MODEL_USE) {
            let seq = unit.stamp(thread_id).await?;
            threads::hold_input(&mut *unit.conn, input.id, Some(HELD_MODEL_USE), seq).await?;
            self.source_requested(
                unit,
                thread,
                orch_db::queries::notifications::inbox::RequestSource::Input(input.id),
                None,
            )
            .await?;
        }
        Ok(())
    }

    /// Checks 2 to 12, in their order. The first that fails is the answer.
    /// Checks 2 to 11 for the input that would start the turn; without an input, what the thread itself stands on
    /// (a block whose cause is the thread's binding or its place is looked at again though nothing waits): no
    /// limit was let past, no recovery trial is reserved, and the thread's owner is the one whose entry counts.
    async fn check_binding(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        input: Option<&InputRow>,
        settings: &Resolved,
    ) -> Result<Result<Plan, Blocked>, EngineError> {
        let now = unit.now();
        let conn = &mut *unit.conn;
        // 2
        let (Some(agent_id), Some(provider_id)) = (thread.agent_id, thread.provider_id) else {
            return Ok(Err(Blocked {
                reason: "unbound",
                detail: None,
            }));
        };
        // 3
        let Some(agent) = resources::agent(conn, agent_id)
            .await?
            .filter(|a| a.deleted_at.is_none())
        else {
            return Ok(Err(Blocked::new(
                "agent_disabled",
                "the agent profile is gone",
            )));
        };
        if agent.disabled {
            let note = crate::resource_facts::activity::note(
                conn,
                &orch_api_contract::resource_facts::FactTarget {
                    object: orch_access_contract::decisions::ObjectRef {
                        kind: orch_access_contract::permissions::ObjectKind::Agent,
                        id: agent.id,
                    },
                    model_id: None,
                },
            )
            .await?;
            return Ok(Err(Blocked {
                reason: "agent_disabled",
                detail: note,
            }));
        }
        // 4
        let Some(mut provider) = resources::provider(conn, provider_id)
            .await?
            .filter(|p| p.deleted_at.is_none())
        else {
            return Ok(Err(Blocked::new("provider_unavailable", "deleted")));
        };
        if let Some(why) =
            crate::provider_policy::forbidden(conn, thread.node_id, provider_id).await?
        {
            return Ok(Err(Blocked::new("provider_not_allowed", why.to_string())));
        }
        if provider.status == "disabled" {
            let note = crate::resource_facts::activity::note(
                conn,
                &orch_api_contract::resource_facts::FactTarget {
                    object: orch_access_contract::decisions::ObjectRef {
                        kind: orch_access_contract::permissions::ObjectKind::Provider,
                        id: provider.id,
                    },
                    model_id: None,
                },
            )
            .await?;
            return Ok(Err(Blocked::new(
                "provider_unavailable",
                note.map_or_else(|| "disabled".into(), |note| format!("disabled · {note}")),
            )));
        }
        // 5. A provider that a limit with a time closed looks at the vendor again when the time has passed.
        if let Some(opened) = crate::providers::release_if_ended(conn, &provider, now).await? {
            provider = opened;
        }
        let mut provider_limit_by = None;
        // A refused provider admits the one recovery trial its manager reserved for an input; nothing else.
        let trial = match input {
            Some(input) if provider.status == "error" => {
                orch_db::queries::provider_facts::trial_allowed(
                    &mut *unit.conn,
                    provider.id,
                    input.id,
                )
                .await?
            }
            _ => false,
        };
        if provider.status == "exhausted" {
            provider_limit_by = match input {
                Some(input) => self.limit_is_passed(unit, input, &provider).await?,
                None => None,
            };
            if provider_limit_by.is_none() {
                return Ok(Err(Blocked {
                    reason: "provider_exhausted",
                    detail: provider.status_detail.clone(),
                }));
            }
        } else if provider.status != "ready" && !trial {
            return Ok(Err(Blocked::new(
                "provider_unavailable",
                crate::providers::unavailable_detail(&provider),
            )));
        }
        let conn = &mut *unit.conn;
        if provider
            .held_by_environment_id
            .is_some_and(|holder| Some(holder) != thread.environment_id)
        {
            return Ok(Err(Blocked::new("provider_unavailable", "handed_over")));
        }
        // 6
        let Some(model) = settings.model.clone() else {
            return Ok(Err(Blocked {
                reason: "model_not_set",
                detail: None,
            }));
        };
        // 7
        let entry = resources::model(conn, provider.id, &model).await?;
        if let Err(why) =
            crate::models::check_allowed(entry.as_ref(), settings.models_allowed.as_deref(), &model)
        {
            let mut detail = why.describe(&model);
            if matches!(why, crate::models::NotAllowed::NotEnabled) {
                let note:Option<String>=sqlx::query_scalar("SELECT pause_note FROM provider_models WHERE provider_id=$1 AND model_name=$2 AND NOT enabled").bind(provider.id).bind(&model).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?.flatten();
                if let Some(note) = note {
                    detail.push_str(" · ");
                    detail.push_str(&note);
                }
            }
            return Ok(Err(Blocked::new("model_not_allowed", detail)));
        }
        let Some(model_row) = entry else {
            return Ok(Err(Blocked::new(
                "model_not_allowed",
                crate::models::NotAllowed::NotEnabled.describe(&model),
            )));
        };
        let parameters: ModelParameters = binding::model_parameters(&model_row)?;
        if parameters.effort_is_gone(settings.effort.as_deref()) {
            return Ok(Err(Blocked::new(
                "model_not_allowed",
                format!(
                    "the model {model} does not take the effort {}",
                    settings.effort.as_deref().unwrap_or_default()
                ),
            )));
        }
        let effort = parameters.effort_for(settings.effort.as_deref());
        // A family of models whose limit stopped this thread's last turn stays closed to it until the vendor's time
        // passes: another model of the plan is what is offered, never the same family again.
        if let Some(blocked) = family_at_its_limit(conn, thread.node_id, &model, now).await? {
            return Ok(Err(blocked));
        }
        // 8, 9, 10, 11 need the environment and the descriptor.
        let descriptor = harnesses::descriptor_of(&agent)?;
        if let Some(descriptor) = &descriptor {
            if !settings.allowed_modes.contains(&settings.approval_mode) {
                return Ok(Err(Blocked::new(
                    "binding_invalid",
                    crate::sessions::APPROVAL_NOT_ALLOWED,
                )));
            }
            if !descriptor.approval_modes.contains(&settings.approval_mode) {
                return Ok(Err(Blocked::new(
                    "binding_invalid",
                    crate::sessions::APPROVAL_UNSUPPORTED,
                )));
            }
        }
        let environment = match thread.environment_id {
            Some(id) => environments::environment(conn, id).await?,
            None => None,
        };
        let Some(environment) = environment.filter(|e| e.deleted_at.is_none()) else {
            return Ok(Err(Blocked::new(
                "environment_unavailable",
                "the thread has no environment",
            )));
        };
        // A physical update holds every binding using its directory, key or hosted unit. The
        // same per-machine transaction locks protect the coordinator's final idle grant.
        if orch_db::queries::component_updates::blocks_binding(conn, environment.id, provider.id)
            .await?
        {
            return Ok(Err(Blocked::new(
                "agent_offline",
                "This machine is updating. Queued work resumes after it reconnects.",
            )));
        }
        // A first refused start already reports its build, before ordinary binding facts are ready.
        if let Some(pin) = agent.profile.get("harness_version").and_then(Json::as_str)
            && let Some(machine) = harnesses::machine_of(
                thread.arrangement.as_deref(),
                &provider,
                environment.machine_id,
            )
            && let Some(reported) = resources::harness(conn, provider.id, agent.id, machine).await?
            && let Some(version) = reported.harness_version
            && version != pin
        {
            return Ok(Err(Blocked::new(
                "binding_invalid",
                format!(
                    "{} {} is running, but this profile requires {}. Update the runtime or choose a matching profile",
                    agent.harness_kind, version, pin
                ),
            )));
        }
        let Some(descriptor) = descriptor else {
            return Ok(Err(Blocked::new(
                "binding_invalid",
                "the agent profile has not reported what its harness is",
            )));
        };
        let machine_of_environment = match environment.machine_id {
            Some(id) => resources::machine_place(conn, id).await?,
            None => None,
        };
        let template = environments::template_spec(conn, environment.id).await?;
        let environment_machine = orch_db::queries::environment_facts::of(conn, environment.id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the environment is gone"))?;
        if let Err(why) = sessions::confinement(
            settings,
            &descriptor,
            &environment,
            template.as_ref(),
            &environment_machine,
            thread.executor.as_deref(),
        ) {
            return Ok(Err(Blocked::new("binding_invalid", why)));
        }
        // 9
        if let Some(why) = self
            .bypass_environment(conn, environment.id, settings.approval_mode)
            .await?
        {
            return Ok(Err(Blocked::new("binding_invalid", why)));
        }
        if settings.approval_mode == orch_agent_contract::descriptor::ApprovalMode::Auto {
            let models = self
                .review_models(
                    conn,
                    thread,
                    provider.id,
                    input.and_then(InputRow::answers),
                    settings,
                )
                .await?;
            let declared = crate::harnesses::descriptor_of(&agent)?;
            if let Err(why) = crate::approval_review::reviewer_allowed(
                settings.approval_mode,
                declared.as_ref(),
                &models,
            ) {
                return Ok(Err(Blocked::new("model_not_allowed", why)));
            }
        }
        if let Some(blocked) =
            environment_unavailable(&environment, machine_of_environment.as_ref())
        {
            return Ok(Err(blocked));
        }
        // A workspace that holds a configuration of the harness's own waits while the people who manage the
        // environment are asked whether the harness may use it; nothing is opened before they answer.
        if let Some(blocked) =
            crate::trust::waiting(conn, &environment, &agent.harness_kind).await?
        {
            return Ok(Err(blocked));
        }
        // 10
        let offers = binding::offers_of(conn, environment.id).await?;
        let colocated = binding::colocated_of(conn, &environment).await?;
        let runtime = binding::runtime_facts(conn, &agent, &provider, &environment).await?;
        let facts = Facts {
            descriptor: &descriptor,
            provider: &provider,
            environment: &environment,
            offers,
            settings,
            entry: self
                .entry_arrangements(unit, thread, input.and_then(InputRow::answers), &provider)
                .await?,
            colocated,
            runtime,
        };
        let conn = &mut *unit.conn;
        let requested = parse_arrangement(thread.arrangement.as_deref());
        // Where the credential is kept on a person's machine, that machine's own list of the types it
        // lets a credential be used in is asked as well: it comes before every page.
        let machine_list = binding::machine_arrangements_of(conn, &provider).await?;
        let permission = binding::policy_with_machine(&facts, machine_list.clone()).permission;
        let arrangement = match binding::choose_with_machine(&facts, machine_list, requested) {
            Ok(arrangement) => arrangement,
            Err(why) => {
                // A role removed from an existing binding is an availability failure.
                // New bindings still reject that placement before a thread exists.
                if why == orch_agent_contract::arrangement::Incompatible::AgentNotOnThatMachine
                    && let Some(kind) = requested
                    && permission.allows(kind).is_ok()
                {
                    if orch_agent_contract::arrangement::credential_path(
                        kind,
                        orch_agent_contract::arrangement::native_matches(
                            facts.descriptor,
                            &binding::serves(&provider),
                        ),
                        binding::native_login_here(kind, &provider, Some(agent.id)),
                    ) == CredentialPath::Forwarded
                        && !crate::provider_runtime::waits_for_runtime(conn, &provider).await?
                    {
                        let keeper = match leases::machine_of(&provider) {
                            Some(id) => resources::machine_place(conn, id).await?,
                            None => None,
                        };
                        if let Some(blocked) = key_place_unavailable(&provider, keeper.as_ref()) {
                            return Ok(Err(blocked));
                        }
                    }
                    if let Some(id) = harnesses::machine_of(
                        Some(kind.as_str()),
                        &provider,
                        environment.machine_id,
                    ) && let Some(machine) = resources::machine_place(conn, id).await?
                        && !machine.roles.iter().any(|role| role == "agent")
                        && let Some(blocked) = machine_unavailable(Some(&machine))
                    {
                        return Ok(Err(blocked));
                    }
                }
                return Ok(Err(Blocked::new(
                    "binding_invalid",
                    binding::describe(&why),
                )));
            }
        };
        let path = binding::selected_path(&arrangement, &provider, &agent);
        // A credential that leaves the provider (the part that keeps providers): the arrangement names
        // the form that leaves, and the login is handed over to one environment at a time, and not
        // while turns of other environments run on the provider.
        if matches!(path, CredentialPath::Shared | CredentialPath::Handover) {
            if arrangement.form.is_none() {
                return Ok(Err(Blocked::new(
                    "binding_invalid",
                    "the provider and the harness have no form of the credential in common",
                )));
            }
            if path == CredentialPath::Handover
                && provider.held_by_environment_id.is_none()
                && orch_db::queries::resources::provider_runtime::turns_outside(
                    conn,
                    provider.id,
                    environment.id,
                )
                .await?
                    > 0
            {
                return Ok(Err(Blocked::new("provider_unavailable", "in_use")));
            }
        }
        // The key is kept somewhere a turn can reach: where the turn goes through the forwarder, the
        // machine that keeps it is there, takes the role, and is online. Its absence is the
        // provider's, and is told as the provider's (`offline`, as a provider whose machine is
        // away stands), before the place the harness runs is asked about (11).
        // A managed provider whose runtime is not up is not asked about its machine: the start of the
        // turn starts the unit and waits for it (the part that keeps providers).
        let provider_starts = crate::provider_runtime::waits_for_runtime(conn, &provider).await?;
        if path == CredentialPath::Forwarded && !provider_starts {
            let keeper = match leases::machine_of(&provider) {
                Some(id) => resources::machine_place(conn, id).await?,
                None => None,
            };
            if let Some(blocked) = key_place_unavailable(&provider, keeper.as_ref()) {
                return Ok(Err(blocked));
            }
        }
        // 11. A harness that runs in the environment is where the environment is: while the environment
        // is brought up by the start sequence there is nothing to ask yet.
        let waits_for_environment =
            starts_by_itself(&environment) && arrangement.kind.as_str() != "separated";
        let machine_id = harnesses::machine_of(
            Some(arrangement.kind.as_str()),
            &provider,
            environment.machine_id,
        );
        let waits_for_provider = provider_starts && arrangement.kind.as_str() == "separated";
        if !waits_for_environment && !waits_for_provider {
            let Some(machine_id) = machine_id else {
                return Ok(Err(Blocked::new(
                    "agent_offline",
                    "the place where the harness runs has no machine: its runtime is not running",
                )));
            };
            let machine = resources::machine_place(conn, machine_id).await?;
            if let Some(blocked) = machine_unavailable(machine.as_ref()) {
                return Ok(Err(blocked));
            }
        }
        // 12 is asked of the caps by `preflight_in`, once the binding holds.
        let params = sessions_params(
            &descriptor,
            &arrangement,
            &provider,
            &agent,
            &environment,
            &parameters,
            settings,
            path,
        );
        let metering = metering(path, descriptor.capabilities.usage_tokens);
        let tier = crate::providers::service_tier(&descriptor.parameters, &params);
        Ok(Ok(Plan {
            agent,
            provider,
            environment,
            model,
            effort,
            params,
            tier,
            arrangement,
            path,
            metering,
            provider_limit_by,
            allowance: None,
            passed: Vec::new(),
            bypass: None,
        }))
    }

    /// The arrangement types the entry the input's author uses the provider
    /// through names, which only narrow what the provider's owner allows.
    async fn entry_arrangements(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        author: Option<Id>,
        provider: &ProviderRow,
    ) -> Result<Option<Vec<ArrangementType>>, EngineError> {
        let person = author.unwrap_or(thread.owner_id);
        let answer = self
            .inner
            .access
            .provider_use(
                &mut *unit.conn,
                &ProviderUse {
                    person,
                    node: thread.node_id,
                    provider: provider.id,
                    model: None,
                    effort: None,
                },
            )
            .await?;
        Ok(match answer {
            Answer::Allowed { arrangements } => arrangements,
            Answer::Refused(_) => None,
        })
    }

    /// Who let the input past the provider's own limit and holds the right to
    /// now; none where nobody does.
    async fn limit_is_passed(
        &self,
        unit: &mut Unit<'_>,
        input: &InputRow,
        provider: &ProviderRow,
    ) -> Result<Option<Id>, EngineError> {
        let entries = input
            .limit_bypass
            .as_ref()
            .and_then(Json::as_array)
            .cloned()
            .unwrap_or_default();
        for entry in entries {
            let passes = entry
                .get("provider_limit")
                .and_then(Json::as_bool)
                .unwrap_or(false);
            let by: Option<Id> = entry
                .get("by")
                .and_then(Json::as_str)
                .and_then(|id| id.parse().ok());
            let as_admin = entry
                .get("as_admin")
                .and_then(Json::as_bool)
                .unwrap_or(false);
            if let (true, Some(by)) = (passes, by) {
                let holds = self
                    .inner
                    .access
                    .may_pass(
                        &mut *unit.conn,
                        by,
                        as_admin,
                        &Limit::ProviderLimit {
                            provider: provider.id,
                        },
                    )
                    .await?;
                if holds {
                    return Ok(Some(by));
                }
            }
        }
        Ok(None)
    }

    /// The success path: everything a started turn is made of, in the
    /// transaction of the checks.
    async fn make_turn(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        input: &InputRow,
        plan: Plan,
    ) -> Result<Started, EngineError> {
        let seq = unit.stamp(thread.node_id).await?;
        let lease = match leases::mode_of(plan.path) {
            // A lease that lets a credential leave is opened by the start sequence of the turn, once
            // the machine where the harness runs is known (the part that keeps providers).
            None
            | Some(
                orch_link_contract::commands::LeaseMode::Shared
                | orch_link_contract::commands::LeaseMode::Handover,
            ) => None,
            Some(mode) => {
                leases::open_for_turn(
                    unit,
                    thread.node_id,
                    plan.provider.id,
                    &plan.model,
                    mode,
                    plan.allowance.clone(),
                )
                .await?
            }
        };
        let segment = sessions::ensure_segment(
            unit,
            &sessions::SegmentPlace {
                thread,
                agent: &plan.agent,
                provider: &plan.provider,
                environment: &plan.environment,
                path: plan.path,
                started_by: input.author_id,
            },
            seq,
        )
        .await?;
        // What follows from the environment's offers is worked out each time.
        if thread.executor.as_deref() != plan.arrangement.executor.as_deref()
            || thread.tool_set.as_deref() != plan.arrangement.tool_set.as_deref()
        {
            threads::set_binding(
                &mut *unit.conn,
                thread.node_id,
                &threads::Binding {
                    agent: plan.agent.id,
                    provider: plan.provider.id,
                    arrangement: plan.arrangement.kind.as_str(),
                    executor: plan.arrangement.executor.as_deref(),
                    tool_set: plan.arrangement.tool_set.as_deref(),
                },
                seq,
            )
            .await?;
        }
        let turn_id = new_id();
        let now = unit.now();
        let turn = threads::insert_turn(
            &mut *unit.conn,
            &NewTurn {
                id: turn_id,
                thread_id: thread.node_id,
                segment_id: segment.id,
                input_id: input.id,
                model: &plan.model,
                effort: plan.effort.as_deref(),
                params: &plan.params,
                lease_id: lease.as_ref().map(|lease| lease.id),
                limit_bypass: plan.bypass.as_ref(),
                at: now,
            },
            seq,
        )
        .await?;
        self.bind_before(unit, thread, input, &plan.environment, &turn)
            .await?;
        orch_db::queries::provider_facts::bind_trial_turn(
            &mut *unit.conn,
            segment.provider_id,
            input.id,
            turn.id,
        )
        .await?;
        if let Some(lease) = &lease {
            threads::set_lease_turn(&mut *unit.conn, lease.id, turn.id).await?;
        }
        // A turn that starts past limits that were reached writes down what each was and who let it by.
        if !plan.passed.is_empty() {
            self.record_passed(
                unit,
                thread.node_id,
                segment.id,
                turn.id,
                input.author_id,
                &plan.passed,
            )
            .await?;
        }
        if let Some(by) = plan.provider_limit_by {
            self.record_passed_provider(
                unit,
                &crate::caps::ProviderPass {
                    thread: thread.node_id,
                    segment: segment.id,
                    turn: turn.id,
                    author: input.author_id,
                    provider: plan.provider.id,
                    by,
                    detail: plan
                        .provider
                        .status_detail
                        .as_deref()
                        .unwrap_or("the plan's limit was reached"),
                },
            )
            .await?;
        }
        let moved = threads::change_input(
            &mut *unit.conn,
            input.id,
            &[input_state::PENDING],
            &InputChange {
                state: input_state::DELIVERING,
                // Files written before the turn are not written again by its start.
                detail: input
                    .state_detail
                    .as_deref()
                    .filter(|detail| *detail == crate::preparation::FILES_READY),
                turn_id: Some(turn.id),
                ..InputChange::default()
            },
            seq,
        )
        .await?;
        if !moved {
            return Err(EngineError::unreadable(
                "an input that waited was taken by another before its turn was made",
            ));
        }
        threads::set_current(
            &mut *unit.conn,
            thread.node_id,
            Some(segment.id),
            Some(turn.id),
            seq,
        )
        .await?;
        threads::set_state(
            &mut *unit.conn,
            thread.node_id,
            &[thread_state::IDLE, thread_state::BLOCKED],
            &StateChange::starting(Some("harness")),
            seq,
        )
        .await?;
        unit.wake(thread.node_id);
        Ok(Started {
            thread: thread.node_id,
            turn: turn.id,
            input: input.id,
            segment: segment.id,
        })
    }
}

/// Whether an environment that is not up is brought up by the start of a turn: one made from a
/// template that has no unit yet, is stopped, or is being made. The turn's start sequence starts it
/// (or waits for the job that does) and says that the thread waits for the environment.
#[must_use]
pub(crate) fn starts_by_itself(environment: &EnvironmentRow) -> bool {
    environment
        .facts
        .pointer("/environment_descriptor/lifecycle")
        .and_then(Json::as_str)
        == Some("unit")
        && environment.remove_at.is_none()
        && matches!(
            environment.status.as_str(),
            "none" | "stopped" | "provisioning"
        )
}

/// Whether a directory on a person's machine that is not up is attached by the schedule once the
/// turn is there to wait for it: its machine has a session and serves environments, and nothing
/// refused it. The start sequence of the turn waits for the attach instead of being blocked.
#[must_use]
fn attached_by_schedule(
    environment: &EnvironmentRow,
    machine: Option<&resources::MachinePlace>,
) -> bool {
    environment
        .facts
        .pointer("/environment_descriptor/lifecycle")
        .and_then(Json::as_str)
        == Some("attach")
        && environment.remove_at.is_none()
        && matches!(environment.status.as_str(), "none" | "offline")
        && machine.is_some_and(|machine| {
            machine.online
                && machine.status == "active"
                && machine.deleted_at.is_none()
                && machine.roles.iter().any(|role| role == "environment")
        })
}

/// What is said of an environment that a turn cannot run in because of the state it is in. Whoever says
/// it, the start of a turn that ends for it or the pre-flight that blocks the thread, says it in these
/// words: said in two, the people who follow the thread are told twice of one thing.
pub(crate) fn environment_state_sentence(status: &str, detail: Option<&str>) -> String {
    match (detail, status) {
        (_, "lost") => "its environment was lost".to_owned(),
        (Some(detail), "failed") => format!("the environment failed: {detail}"),
        (Some(detail), "disabled") => format!("disabled · {detail}"),
        (None, "disabled") => "disabled".into(),
        _ => format!("the environment is {status}"),
    }
}

/// Whether the limit of one family of models stopped the thread's last turn and still holds `model`: until the time
/// the vendor named, every model of that family (by the family the adapter read from the vendor's fields), or the
/// same model where no family was named. The thread waits as one whose provider is at its limit, and what is offered
/// is another model of the plan.
async fn family_at_its_limit(
    conn: &mut orch_db::Conn,
    thread: Id,
    model: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Option<Blocked>, EngineError> {
    let Some(last) = threads::latest_turn(conn, thread).await? else {
        return Ok(None);
    };
    let Some(error) = last
        .error
        .as_ref()
        .filter(|_| last.state == threads::turn_state::FAILED)
    else {
        return Ok(None);
    };
    if error["code"] != "provider_exhausted" || error["limit"]["scope"] != "model_family" {
        return Ok(None);
    }
    let until = error["resets_at"]
        .as_str()
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map(|at| at.with_timezone(&chrono::Utc));
    if until.is_some_and(|until| until <= now) {
        return Ok(None);
    }
    let family = error["limit"]["model_family"].as_str();
    let holds = match family {
        Some(family) => model.to_lowercase().contains(&family.to_lowercase()),
        None => model == last.model,
    };
    if !holds {
        return Ok(None);
    }
    let ends = until.map_or_else(String::new, |until| {
        format!(" until {}", until.format("%Y-%m-%d %H:%M UTC"))
    });
    let sentence = match family {
        Some(family) => {
            format!("the plan's {family} models are at their limit{ends}; choose another model")
        }
        None => format!("the plan's {model} is at its limit{ends}; choose another model"),
    };
    Ok(Some(Blocked::new("provider_exhausted", sentence)))
}

/// Check 9 on its own: is the environment one a turn can run in now?
fn environment_unavailable(
    environment: &EnvironmentRow,
    machine: Option<&resources::MachinePlace>,
) -> Option<Blocked> {
    if environment.remove_at.is_some() {
        return Some(Blocked::new(
            "environment_unavailable",
            "its removal is scheduled",
        ));
    }
    if starts_by_itself(environment) || attached_by_schedule(environment, machine) {
        return None;
    }
    if environment.status != "running" {
        return Some(Blocked::new(
            "environment_unavailable",
            environment_state_sentence(&environment.status, environment.status_detail.as_deref()),
        ));
    }
    match machine {
        None => Some(Blocked::new(
            "environment_unavailable",
            "the environment has no machine",
        )),
        Some(machine)
            if !machine.online || machine.status != "active" || machine.deleted_at.is_some() =>
        {
            Some(Blocked::new(
                "environment_unavailable",
                "the environment's machine is offline",
            ))
        }
        Some(_) => None,
    }
}

/// Can the machine that keeps the provider's key serve a model call now? The thread waits as
/// `provider_unavailable` when it cannot, with the sentence `offline` for a machine that is away, which
/// is also how a provider whose machine is away stands.
fn key_place_unavailable(
    provider: &ProviderRow,
    machine: Option<&resources::MachinePlace>,
) -> Option<Blocked> {
    if leases::machine_of(provider).is_none() {
        return Some(Blocked::new(
            "provider_unavailable",
            "the provider's key is kept on no machine; choose a machine to keep it",
        ));
    }
    match machine {
        None => Some(Blocked::new(
            "provider_unavailable",
            "the machine that keeps the key is gone",
        )),
        Some(machine) if machine.status != "active" || machine.deleted_at.is_some() => {
            Some(Blocked::new(
                "provider_unavailable",
                "the machine that keeps the key was revoked",
            ))
        }
        Some(machine) if !machine.online => Some(Blocked::new("provider_unavailable", "offline")),
        Some(machine) if !machine.roles.iter().any(|role| role == "provider") => {
            Some(Blocked::new(
                "provider_unavailable",
                "the machine that keeps the key does not take the role of a provider",
            ))
        }
        Some(_) => None,
    }
}

/// Check 11 on its own: can the machine where the harness runs run it now?
fn machine_unavailable(machine: Option<&resources::MachinePlace>) -> Option<Blocked> {
    match machine {
        None => Some(Blocked::new(
            "agent_offline",
            "the machine of the harness is gone",
        )),
        Some(machine) if machine.status != "active" || machine.deleted_at.is_some() => Some(
            Blocked::new("agent_offline", "the machine of the harness was revoked"),
        ),
        Some(machine) if !machine.online => Some(Blocked::new(
            "agent_offline",
            "the machine of the harness is offline",
        )),
        Some(machine) if !machine.roles.iter().any(|role| role == "agent") => Some(Blocked::new(
            "agent_offline",
            "the machine of the harness does not take the role of an agent",
        )),
        Some(_) => None,
    }
}

/// The nodes from the root org down to a thread.
async fn nodes_path(conn: &mut orch_db::Conn, thread: Id) -> Result<Vec<Id>, EngineError> {
    Ok(threads::node_path(conn, thread).await?)
}

/// Whether the team org the node `node` (a thread's) lies in requires a second step of `author`, who has none
/// set up: they are a person (a service identity has no human step), a member of that tree, without an
/// authenticator or a mailed code switched on, and `auth.second_step` resolves to `required` at the tree's root
/// org. An installation where nothing requires the step pays one read.
pub(crate) async fn second_step_missing(
    conn: &mut orch_db::Conn,
    author: Id,
    node: Id,
) -> Result<bool, EngineError> {
    use orch_db::queries::second_step as rows;
    if !rows::required_anywhere(conn).await? {
        return Ok(false);
    }
    let Some(person) = orch_db::queries::identity::user_by_id(conn, author).await? else {
        return Ok(false);
    };
    if person.kind == "service" || person.totp_on() || person.email_code_on() {
        return Ok(false);
    }
    let Some(root) = threads::node_path(conn, node).await?.first().copied() else {
        return Ok(false);
    };
    let member = rows::root_orgs_of(conn, &[author])
        .await?
        .iter()
        .any(|(_, org)| *org == root);
    if !member {
        return Ok(false);
    }
    let def = setting_defs::get(setting_keys::AUTH_SECOND_STEP)
        .ok_or_else(|| EngineError::unreadable("the second step setting is not registered"))?;
    let snapshot = Snapshot::load(conn, Place::Node(root), Wanted::Keys(&[def])).await?;
    Ok(snapshot.resolve(def).value.as_ref().and_then(Json::as_str) == Some(SECOND_STEP_REQUIRED))
}

impl Engine {
    /// A person's second step was set up, or a place stopped requiring one: the threads where inputs wait held
    /// for it (of `author` alone, where one is named) are looked at, and their pre-flight releases what may go.
    pub async fn reevaluate_second_step(&self, author: Option<Id>) -> Result<usize, EngineError> {
        let held = {
            let mut conn = self.inner.db.acquire().await?;
            threads::threads_held_for_second_step(&mut conn, author).await?
        };
        for thread in &held {
            self.wake(*thread);
        }
        Ok(held.len())
    }
}

/// The parameters of the adapter that a turn is sent with.
#[allow(
    clippy::too_many_arguments,
    reason = "the facts a turn's parameters are resolved from"
)]
pub(crate) fn sessions_params(
    descriptor: &AgentDescriptor,
    arrangement: &Arrangement,
    provider: &ProviderRow,
    _agent: &AgentRow,
    environment: &EnvironmentRow,
    parameters: &ModelParameters,
    settings: &Resolved,
    path: CredentialPath,
) -> Json {
    use orch_agent_contract::descriptor::Placement;
    use orch_agent_contract::parameters::{Eligibility, sendable};
    let _ = arrangement;
    let context = Eligibility {
        credential_path: path,
        provider_kind: binding::provider_kind(provider),
        environment_placement: if environment
            .facts
            .pointer("/environment_descriptor/lifecycle")
            .and_then(Json::as_str)
            == Some("unit")
        {
            Placement::Managed
        } else {
            Placement::Connected
        },
        model: parameters.clone(),
        harness_version: None,
    };
    let sent = sendable(
        &descriptor.parameters,
        &context,
        &settings.features,
        &settings.params,
    );
    Json::Object(sent.values.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn environment(status: &str) -> EnvironmentRow {
        EnvironmentRow {
            id: new_id(),
            name: "env".into(),
            owner_id: new_id(),
            org_id: new_id(),
            placement: "managed".into(),
            home_node_id: new_id(),
            workspace_root: "/workspace".into(),
            config: Json::Null,
            machine_id: Some(new_id()),
            generation: 1,
            status: status.into(),
            status_detail: None,
            facts: serde_json::json!({"environment_descriptor":{"lifecycle":"unit"}}),
            remove_at: None,
            deleted_at: None,
        }
    }

    fn machine(online: bool) -> resources::MachinePlace {
        resources::MachinePlace {
            id: new_id(),
            roles: vec!["agent".into()],
            status: "active".into(),
            online,
            placement: "connected".into(),
            owner_id: None,
            facts: Json::Null,
            deleted_at: None,
        }
    }

    #[test]
    fn an_environment_is_available_when_it_runs_and_its_machine_is_online() {
        assert_eq!(
            environment_unavailable(&environment("running"), Some(&machine(true))),
            None
        );
        for status in ["failed", "offline", "recreating", "moving", "disabled"] {
            let blocked =
                environment_unavailable(&environment(status), Some(&machine(true))).unwrap();
            assert_eq!(blocked.reason, "environment_unavailable", "{status}");
        }
        // An environment made from a template that is not up is brought up by the turn's start.
        for status in ["none", "stopped", "provisioning"] {
            assert_eq!(
                environment_unavailable(&environment(status), None),
                None,
                "{status}"
            );
        }
        assert!(environment_unavailable(&environment("running"), Some(&machine(false))).is_some());
        assert!(environment_unavailable(&environment("running"), None).is_some());
        let mut removing = environment("running");
        removing.remove_at = Some(Utc::now());
        assert!(environment_unavailable(&removing, Some(&machine(true))).is_some());
    }

    #[test]
    fn a_directory_on_a_persons_machine_waits_for_its_machine_to_attach_it_unless_the_machine_is_away_or_refused_it()
     {
        let mut directory = environment("none");
        directory.placement = "connected".into();
        directory.facts = serde_json::json!({"environment_descriptor":{"lifecycle":"attach"}});
        let mut serving = machine(true);
        serving.roles = vec!["agent".into(), "environment".into()];
        for status in ["none", "offline"] {
            directory.status = status.into();
            assert_eq!(
                environment_unavailable(&directory, Some(&serving)),
                None,
                "{status}: the schedule attaches it and the turn waits"
            );
        }
        let mut away = serving.clone();
        away.online = false;
        assert!(environment_unavailable(&directory, Some(&away)).is_some());
        assert!(environment_unavailable(&directory, None).is_some());
        let not_serving = machine(true);
        assert!(
            environment_unavailable(&directory, Some(&not_serving)).is_some(),
            "a machine that does not take the role of an environment cannot attach it"
        );
        directory.status = "failed".into();
        directory.status_detail = Some("its machine refused it".into());
        let refused = environment_unavailable(&directory, Some(&serving)).unwrap();
        assert!(refused.detail.unwrap_or_default().contains("refused"));
    }

    fn provider_on(machine: Option<Id>, key_machine: Option<Id>) -> ProviderRow {
        ProviderRow {
            id: new_id(),
            name: "provider".into(),
            owner_id: new_id(),
            org_id: new_id(),
            home_id: new_id(),
            kind: "api".into(),
            harness_kind: None,
            agent_id: None,
            protocols: vec!["openai-responses".into()],
            placement: "connected".into(),
            machine_id: machine,
            key_machine_id: key_machine,
            arrangements: vec!["key_separated".into()],
            held_by_environment_id: None,
            status: "ready".into(),
            status_detail: None,
            sources: serde_json::json!({}),
            facts: Json::Null,
            revision: 1,
            deleted_at: None,
        }
    }

    fn keeper(online: bool) -> resources::MachinePlace {
        let mut keeper = machine(online);
        keeper.roles = vec!["provider".into()];
        keeper
    }

    #[test]
    fn the_machine_that_keeps_the_key_must_be_there_online_and_take_the_role_of_a_provider() {
        let machine = new_id();
        let provider = provider_on(Some(machine), Some(machine));
        assert_eq!(key_place_unavailable(&provider, Some(&keeper(true))), None);
        // A machine that is away stands as `offline`, the sentence a provider whose machine is away has.
        let away = key_place_unavailable(&provider, Some(&keeper(false))).unwrap();
        assert_eq!(away.reason, "provider_unavailable");
        assert_eq!(away.detail.as_deref(), Some("offline"));
        // Each of the other ways to be unfit has a sentence of its own.
        let mut revoked = keeper(true);
        revoked.status = "revoked".into();
        let mut deleted = keeper(true);
        deleted.deleted_at = Some(Utc::now());
        let mut agent_only = keeper(true);
        agent_only.roles = vec!["agent".into(), "environment".into()];
        let mut details = std::collections::BTreeSet::new();
        for unfit in [
            key_place_unavailable(&provider, None),
            key_place_unavailable(&provider, Some(&revoked)),
            key_place_unavailable(&provider, Some(&deleted)),
            key_place_unavailable(&provider, Some(&agent_only)),
        ] {
            let blocked = unfit.unwrap();
            assert_eq!(blocked.reason, "provider_unavailable");
            details.insert(blocked.detail);
        }
        assert_eq!(
            details.len(),
            3,
            "gone, revoked or deleted, no role: {details:?}"
        );
    }

    #[test]
    fn a_provider_requires_a_key_keeper_independently_of_its_runtime_machine() {
        for provider in [provider_on(None, None), provider_on(Some(new_id()), None)] {
            let blocked = key_place_unavailable(&provider, Some(&keeper(true))).unwrap();
            assert_eq!(blocked.reason, "provider_unavailable");
            assert_eq!(
                blocked.detail.as_deref(),
                Some("the provider's key is kept on no machine; choose a machine to keep it")
            );
        }
        // The key's machine is the place that counts, whatever the runtime's machine is.
        let both = provider_on(Some(new_id()), Some(new_id()));
        assert_eq!(leases::machine_of(&both), both.key_machine_id);
        assert_eq!(key_place_unavailable(&both, Some(&keeper(true))), None);
    }

    #[test]
    fn a_harness_can_run_on_an_active_online_machine_with_the_agent_role() {
        assert_eq!(machine_unavailable(Some(&machine(true))), None);
        assert_eq!(
            machine_unavailable(Some(&machine(false))).unwrap().reason,
            "agent_offline"
        );
        let mut without_role = machine(true);
        without_role.roles = vec!["environment".into()];
        assert_eq!(
            machine_unavailable(Some(&without_role)).unwrap().reason,
            "agent_offline"
        );
        let mut revoked = machine(true);
        revoked.status = "revoked".into();
        assert_eq!(
            machine_unavailable(Some(&revoked)).unwrap().reason,
            "agent_offline"
        );
    }
}
