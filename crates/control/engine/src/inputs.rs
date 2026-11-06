//! Queue, steer, interrupt and stop by whoever may drive the thread, attribution and the author's own access, forwarded messages, pending announcements and share notices, and inputs no person sent, asked of the thread's first owner.
//!
//! An input is what a person says to an agent. It is accepted when its sender
//! may drive the thread (`thread.interact`) and may use the thread's provider
//! with its model and effort, and the environment it runs in (`environment.use`:
//! the agent acts there for them), themself; what they lack is told whole. A
//! repeat of the same `client_id` by the same sender returns the input that
//! stands. The input is queued, steered into the turn that runs, or placed
//! first and the turn stopped, as its delivery says, and a pre-flight runs in
//! the same transaction wherever a turn might start.
//! While a person's Stop holds the queue, new arrivals join its end and inherit that
//! hold. Sending work never resumes the queue; only Resume or removing its last input does.
//!
//! **One input starts one turn**, and inputs are never merged. The order of
//! the inputs that wait is theirs to change: whoever may drive the thread
//! orders and cancels what waits, their own and other people's; only the
//! words of an input stay its author's.
//!
//! **Not here.** Inputs without an author (announcements, notices of the
//! system) are queued by the parts that make them, through [`Engine::queue_system_input`];
//! what a pending announcement or share notice adds to the text of a turn is left
//! out until those parts exist (see `instructions`).

use crate::access::ProviderUse;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::preflight::Outcome as PreflightOutcome;
use crate::unit::Unit;
use crate::views::{self, People};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::{Decision, ObjectRef, Refusal};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::threads::{
    ContentPart, Delivery, INPUT_TEXT_MAX_CHARS, LimitBypassRequest, ThreadInput,
    ThreadInputUpdateBody, ThreadInputUpdateReply,
};
use orch_authz::{Act, Caller, Principal, nodes};
use orch_chat::following::BlobOrigin;
use orch_db::audit::{self, Actor, Origin};
use orch_db::queries::threads::{
    self, InputChange, InputRow, NewInput, Place, ThreadRow, TurnRow, input_state, thread_state,
};
use orch_types::{Clock, Hash, Id, Json, Seq, Time, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;

mod authority;
mod authority_work;
/// Current authority for one actual retained input command before first native publication.
pub mod native_delivery;
mod scheduled_source;
pub(crate) use scheduled_source::scheduled_source_current_callers;
pub use scheduled_source::{
    retain_scheduled_source_origin, scheduled_source_authorized, scheduled_source_before_acceptance,
};
/// Shared current input effect binding, reused by HTTP and continuing work.
pub mod operation;
pub(crate) use authority::{InputAuthority, InputCallers};

/// The actual schedule producer branch, retained as intent rather than inferred from a later edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledInputDestination {
    /// The definition selected this existing thread.
    ExistingThread,
    /// The ordinary schedule producer created this occurrence's new thread.
    NewThread,
}

/// Stable existing row references for one real accepted occurrence. No ancestry or admission is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledInputOrigin {
    /// Actual retained definition row.
    pub schedule_id: Id,
    /// Actual occurrence inserted before acceptance.
    pub run_id: Id,
    /// The producer branch used at acceptance, unaffected by later definition edits.
    pub destination: ScheduledInputDestination,
}

/// Actual accepted chat/message/recipient identities for one agent ask. No text, names, ancestry,
/// or client-chosen strings establish this relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatInputOrigin {
    /// The posted message's actual chat.
    pub chat_id: Id,
    /// The posted message whose ordinary agent ask produced the input.
    pub message_id: Id,
    /// The actual recipient of this individual input.
    pub thread_id: Id,
    /// The input allocated by that producer; derived work retains this original association.
    pub input_id: Id,
}

/// Server-verified identities selected by the ordinary forward preparation. Copied text and names
/// are not authority, and grouped/nested copies need not have one output part per selected ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ForwardInputSource {
    /// Messages verified in this actual chat.
    Chat {
        /// The verified source chat.
        chat_id: Id,
        /// The exact selected message identities, not IDs reconstructed from nested copies.
        message_ids: Vec<Id>,
    },
    /// Items verified in this actual thread.
    Thread {
        /// The thread whose ordinary readable projection supplied the items.
        thread_id: Id,
        /// Exact selected item identities; a copied turn may combine several items.
        item_ids: Vec<Id>,
    },
}

/// The original allocated input and exact source selection of one forwarded agent delivery.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForwardInputOrigin {
    pub source: ForwardInputSource,
    pub thread_id: Id,
    pub input_id: Id,
}

/// Actual copy/source/recipient identities for its separately accepted authored comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SnapshotCommentOrigin {
    pub copy_id: Id,
    pub source: crate::sharing::Subject,
    pub thread_id: Id,
    pub input_id: Id,
}

impl SnapshotCommentOrigin {
    fn source_of(
        copy: &orch_db::queries::shares::CopyRow,
    ) -> Result<crate::sharing::Subject, EngineError> {
        match copy.object_kind.as_str() {
            "node" => Ok(crate::sharing::Subject::Node(copy.object_id)),
            "chat" => Ok(crate::sharing::Subject::Chat(copy.object_id)),
            "document" => Ok(crate::sharing::Subject::Document(copy.object_id)),
            _ => Err(EngineError::unreadable(
                "the retained copy has an unknown source identity",
            )),
        }
    }
}

pub(crate) enum ForwardAcceptance {
    New(ForwardInputOrigin),
    Replay(Id),
}

/// What a person sends.
#[derive(Debug, Clone)]
pub struct InputRequest {
    /// What they say.
    pub content: Vec<ContentPart>,
    /// How it reaches the agent.
    pub delivery: Delivery,
    /// What makes sending it again harmless: the same sender and the same
    /// value give the input that stands.
    pub client_id: String,
    /// The limits the sender lets it past.
    pub bypass: Option<LimitBypassRequest>,
    /// Whether it is worked in plan mode: the agent reads and proposes a plan, and changes nothing until
    /// somebody proceeds. Only where the thread's agent program declares `plan_mode`.
    pub plan: bool,
}

/// Where an input that is no person's typing came from: a goal's loop makes inputs in the name of the person who set
/// the goal, with a source of their own and the goal they belong to.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Made<'a> {
    /// The source word stored with the input: `user`, or `goal`.
    pub source: &'a str,
    /// The goal the input belongs to, with what the goal said of itself in it.
    pub goal: Option<(Id, &'a Json)>,
}

impl Made<'_> {
    /// What a person typed.
    pub(crate) const USER: Made<'static> = Made {
        source: "user",
        goal: None,
    };
}

/// One ordinary writer can accept a direct caller or a checked bundle for exactly this work.
/// Identity is shared; every permission/content/native check still runs for every origin.
enum InputExecution<'a> {
    Direct(&'a Caller),
    Origins(&'a InputCallers),
}

impl InputExecution<'_> {
    fn user(&self) -> Id {
        match self {
            Self::Direct(caller) => caller.user(),
            Self::Origins(callers) => callers.user(),
        }
    }
    fn callers(&self) -> &[Caller] {
        match self {
            Self::Direct(caller) => std::slice::from_ref(*caller),
            Self::Origins(callers) => callers.callers(),
        }
    }
    fn captured(&self) -> Result<Json, EngineError> {
        match self {
            Self::Direct(caller) => input_authority(caller),
            Self::Origins(callers) => serde_json::to_value(callers.captured()).map_err(Into::into),
        }
    }
}

struct PreparedInputWork {
    serving: Serving,
    verified: Option<orch_api_contract::native::NativeAction>,
    bypass: Option<Json>,
    content: Vec<ContentPart>,
}

/// An input that was accepted, or found again.
#[derive(Debug, Clone, PartialEq)]
pub struct Submitted {
    /// The input as it stands.
    pub input: ThreadInput,
    /// The highest number of the thread's sequence so far.
    pub seq: Seq,
    /// False when the sender had sent it before.
    pub created: bool,
}

/// The accepted input identity and outcome, with its full view only when the calling producer
/// requested disclosure. This is an internal response choice, never input authority.
#[derive(Debug, Clone, PartialEq)]
pub struct SubmittedProjection {
    /// The newly accepted or previously accepted input.
    pub input_id: Id,
    /// The thread that accepted it.
    pub thread_id: Id,
    /// The ordinary view; absent for receipt-only responses.
    pub input: Option<ThreadInput>,
    /// The highest number of the thread's sequence so far.
    pub seq: Seq,
    /// False when the sender had sent it before.
    pub created: bool,
}

impl SubmittedProjection {
    fn full(self) -> Result<Submitted, InputError> {
        let input = self.input.ok_or_else(|| {
            EngineError::unreadable(
                "an ordinary input producer did not request its full projection",
            )
        })?;
        Ok(Submitted {
            input,
            seq: self.seq,
            created: self.created,
        })
    }
}

/// Trusted current-transaction operands resolved by the shared input operation planner. These
/// facts carry no independent authority and are never serialized into a request or saved grant.
#[derive(Debug, Clone)]
pub struct InputSourceOperands {
    /// Exact admitted source alternatives for each supplied blob hash.
    pub blobs: Vec<(Hash, BlobOrigin)>,
    /// An acceptance found during preparation; it must still be that acceptance in the writer.
    pub replayed: Option<Id>,
    /// Actual cap/provider homes whose bypass was admitted at this checkpoint.
    pub bypass_subjects: Vec<orch_authz::caps::Home>,
    /// The exact current selected native authority, checked again by the writer before any invocation.
    pub native_permission: Option<orch_api_contract::native::NativePermission>,
}

fn bypass_was_prepared(
    selected: &[orch_authz::caps::Home],
    reached: orch_authz::caps::Home,
) -> bool {
    use orch_authz::caps::Home;
    // check_pass asks only the provider for a provider-defined cap; its additional node is not
    // another bypass authority. Node and System homes keep their exact existing meaning.
    selected.iter().any(|selected| match (*selected, reached) {
        (Home::Provider { provider: a, .. }, Home::Provider { provider: b, .. }) => a == b,
        (Home::Node(a), Home::Node(b)) => a == b,
        (Home::System, Home::System) => true,
        _ => false,
    })
}

/// The existing writer's result before any response content or display identities are constructed.
struct TakenRow {
    row: InputRow,
    seq: Seq,
    created: bool,
    interrupt: Option<Id>,
}

impl TakenRow {
    async fn project(
        self,
        conn: &mut orch_db::Conn,
        receipt_only: bool,
    ) -> Result<(SubmittedProjection, Option<Id>), InputError> {
        self.project_with_people(conn, receipt_only, None).await
    }

    async fn project_with_people(
        self,
        conn: &mut orch_db::Conn,
        receipt_only: bool,
        people: Option<Vec<Id>>,
    ) -> Result<(SubmittedProjection, Option<Id>), InputError> {
        let input = if receipt_only {
            None
        } else {
            let ids = people.unwrap_or_else(|| {
                self.row
                    .author_id
                    .into_iter()
                    .chain(self.row.run_as)
                    .collect()
            });
            let people = People::load(conn, ids)
                .await?
                .with_approvals(conn, self.row.approval_id)
                .await?;
            Some(views::input_view(&self.row, &people)?)
        };
        Ok((
            SubmittedProjection {
                input_id: self.row.id,
                thread_id: self.row.thread_id,
                input,
                seq: self.seq,
                created: self.created,
            },
            self.interrupt,
        ))
    }
}

/// An input that was taken inside a unit of work, with what is left for after its commit.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Taken {
    /// The input as it stands.
    pub submitted: Submitted,
    /// The turn the input interrupted, whose interrupt is driven once the transaction has committed.
    pub interrupt: Option<Id>,
}

/// Why an input could not be taken.
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    /// An explicit native selector is invalid or unavailable; never replaced with literal text.
    #[error(transparent)]
    Native(#[from] crate::native_actions::NativeError),
    /// There is no such thread, or no such input in it.
    #[error("there is no such thread or input")]
    NotFound,
    /// The sender lacks something, and this is all of it.
    #[error("the sender lacks what an input needs")]
    Denied(Box<Refusal>),
    /// The thread, or a place above it, is archived.
    #[error("the thread is archived")]
    Archived,
    /// Nothing was said.
    #[error("an input says something")]
    Empty,
    /// More was said than an input may hold.
    #[error("an input holds at most {INPUT_TEXT_MAX_CHARS} characters of text")]
    TooLong,
    /// The request is not one that can be made.
    #[error("{0}")]
    Invalid(&'static str),
    /// The agent does not take a message into a turn that runs.
    #[error("the agent does not take a steer")]
    SteerUnsupported,
    /// The thread's agent program has no plan mode.
    #[error("the agent program has no plan mode")]
    PlanUnsupported,
    /// The input is not waiting any more: it is in `state` (taken up, delivered, cancelled, failed).
    #[error("the input is not waiting any more")]
    NotPending {
        /// The state the input is in.
        state: String,
    },
    /// Somebody already accepted the source request; the recorded result is shown to every decider.
    #[error("the request was decided already")]
    AlreadyDecided(Box<orch_api_contract::approvals::RequestDecisionStatus>),
    /// The words of an input are its author's.
    #[error("only the author changes the words of an input")]
    NotTheAuthor,
    /// A resume named a revision of the queue that is no longer the one that stands.
    #[error("the queue changed since it was seen")]
    StaleQueue {
        /// The revision that stands.
        revision: i64,
    },
    /// A pending input's execution controls cannot drive another continuation's lane.
    #[error("select this input's retained history before changing its delivery or native command")]
    InactiveContinuation {
        /// The actual immutable queue owner.
        continuation_id: Id,
        /// The current execution owner.
        active_continuation_id: Id,
    },
    /// A retry of an input that its files do not hold.
    #[error("the input is not held for its files")]
    NotHeld {
        /// The state the input is in.
        state: String,
        /// What holds it, if anything.
        detail: Option<String>,
    },
    /// Something the engine stands on failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<orch_db::DbError> for InputError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_authz::Error> for InputError {
    fn from(error: orch_authz::Error) -> Self {
        match error {
            orch_authz::Error::Archived { .. } => Self::Archived,
            other => Self::Internal(other.into()),
        }
    }
}

/// The people a record of bypasses names.
fn bypass_authors(row: &InputRow) -> Vec<Id> {
    row.limit_bypass
        .as_ref()
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .get("by")
                .and_then(Json::as_str)
                .and_then(|id| id.parse().ok())
        })
        .collect()
}

/// The characters of text an input holds.
fn text_chars(content: &[ContentPart]) -> usize {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => text.chars().count(),
            ContentPart::Forwarded(forwarded) => forwarded.text.chars().count(),
            _ => 0,
        })
        .sum()
}

/// One literal-input supplier is shared by direct sends and prepared prompt versions.
pub(crate) fn validate_literal_input(request: &InputRequest) -> Result<(), InputError> {
    if request.content.is_empty()
        || text_chars(&request.content) == 0
            && !request
                .content
                .iter()
                .any(|part| !matches!(part, ContentPart::Text { .. }))
    {
        return Err(InputError::Empty);
    }
    if text_chars(&request.content) > usize::try_from(INPUT_TEXT_MAX_CHARS).unwrap_or(usize::MAX) {
        return Err(InputError::TooLong);
    }
    if request.client_id.trim().is_empty() {
        return Err(InputError::Invalid(
            "an input carries the value that makes sending it again safe",
        ));
    }
    Ok(())
}

fn validate_request(
    request: &InputRequest,
    selection: Option<&orch_api_contract::input_schedules::NativeInvocation>,
) -> Result<(), InputError> {
    if selection.is_none() {
        validate_literal_input(request)?;
    }
    if text_chars(&request.content) > usize::try_from(INPUT_TEXT_MAX_CHARS).unwrap_or(usize::MAX) {
        return Err(InputError::TooLong);
    }
    if request.client_id.trim().is_empty() {
        return Err(InputError::Invalid(
            "an input carries the value that makes sending it again safe",
        ));
    }
    Ok(())
}

fn delivery_word(delivery: Delivery) -> &'static str {
    match delivery {
        Delivery::Queue => "queue",
        Delivery::Steer => "steer",
        Delivery::Interrupt => "interrupt",
    }
}

/// How the author of a new input is served for the thread's model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Serving {
    /// They may use the model themselves.
    Own,
    /// They may drive the thread but not use its model: the input waits for somebody who may (`held`), or runs
    /// on the access of the thread's first owner.
    WithoutModel {
        /// Whether it waits, held, for somebody who may use the model.
        held: bool,
    },
}

/// Accepted authorship and the separately approved execution actor of a derived input.
#[derive(Debug, Clone, PartialEq)]
pub struct InputProvenance {
    /// The original author remains named.
    pub author_id: Option<Id>,
    /// Their immutable credential narrowing, including later restrictive edits.
    pub submission_authority: Option<Json>,
    /// The person explicitly answering for the model turn.
    pub run_as: Option<Id>,
    /// That actor's credential boundary, never reconstructed from their user id.
    pub run_as_authority: Option<Json>,
}

/// Derives work from its origin without borrowing browser authority. An explicit approver must first
/// pass the current thread/model checks; automatic producers pass `None` and retain the origin's actor.
pub fn derived_input_provenance(
    origin: &InputRow,
    approved_by: Option<&Caller>,
) -> Result<InputProvenance, EngineError> {
    if origin.author_id.is_none() && !origin.is_authorless() {
        return Err(EngineError::unreadable(
            "the input's author is no longer available",
        ));
    }
    let mut provenance = InputProvenance {
        author_id: origin.author_id,
        submission_authority: origin.submission_authority.clone(),
        run_as: origin.run_as,
        run_as_authority: origin.run_as_authority.clone(),
    };
    if let Some(caller) = approved_by {
        provenance.run_as = Some(caller.user());
        provenance.run_as_authority = Some(input_authority(caller)?);
    }
    Ok(provenance)
}

/// Records exact accepted operation and full immutable credential bounds.
pub(crate) fn input_authority(caller: &Caller) -> Result<Json, EngineError> {
    authority::capture(caller)
}

pub(crate) fn saved_authority(raw: Option<&Json>) -> Result<Option<InputAuthority>, EngineError> {
    authority::saved(raw)
}

pub(crate) fn narrowed_input_authority(
    raw: Option<&Json>,
    caller: &Caller,
) -> Result<Option<Json>, EngineError> {
    authority::narrowed(raw, caller)
}

/// Re-resolve the same current materialized work for every exact accepted operation.
pub(crate) async fn input_authority_callers(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    input: &InputRow,
    actor: Id,
    authority: &InputAuthority,
    clock: &dyn Clock,
) -> Result<Option<InputCallers>, EngineError> {
    authority_work::for_input(unit, thread, input, actor, authority, clock).await
}

pub(crate) async fn input_authority_for_request(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    request: &InputRequest,
    selection: Option<&orch_api_contract::input_schedules::NativeInvocation>,
    actor: Id,
    authority: &InputAuthority,
    clock: &dyn Clock,
) -> Result<Option<InputCallers>, EngineError> {
    authority_work::for_request(unit, thread, request, selection, actor, authority, clock).await
}

/// Scope preparation only for preflight's ordinary current-person assessment. Execution callers
/// are obtained separately after the complete materialized footprint is known.
pub(crate) async fn input_preparation_callers(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    input: &InputRow,
    actor: Id,
    authority: &InputAuthority,
    clock: &dyn Clock,
) -> Result<Option<Vec<Caller>>, EngineError> {
    authority_work::for_preflight(unit, thread, input, actor, authority, clock).await
}

/// The recorded execution actor; an explicit approver never replaces the author's original provenance.
pub(crate) async fn input_caller(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    input: &InputRow,
    clock: &dyn Clock,
) -> Result<Option<InputCallers>, EngineError> {
    let (person, raw) = match input.run_as {
        Some(person) => (person, input.run_as_authority.as_ref()),
        None => match input.author_id {
            Some(person) => (person, input.submission_authority.as_ref()),
            None if input.is_authorless() => {
                return InputCallers::single(
                    Caller::session(thread.owner_id).with_admin_mode(false),
                )
                .map(Some);
            }
            None => return Ok(None),
        },
    };
    let Some(authority) = saved_authority(raw)? else {
        return Ok(None);
    };
    input_authority_callers(unit, thread, input, person, &authority, clock).await
}

/// The author's original boundary, independent of any later model-use approver.
pub(crate) async fn input_author_caller(
    unit: &mut Unit<'_>,
    node: Id,
    input: &InputRow,
    clock: &dyn Clock,
) -> Result<Option<InputCallers>, EngineError> {
    let Some(author) = input.author_id else {
        return Ok(None);
    };
    let Some(authority) = saved_authority(input.submission_authority.as_ref())? else {
        return Ok(None);
    };
    let Some(thread) = threads::thread(unit.conn, node).await? else {
        return Ok(None);
    };
    input_authority_callers(unit, &thread, input, author, &authority, clock).await
}

/// The existing ordinary input/turn driving policy over its supplied resource port.
/// Publication and Engine writers share one implementation; no origin is substituted.
async fn check_sender_for_turn(
    unit: &mut Unit<'_>,
    caller: &Caller,
    thread: &ThreadRow,
    need_provider: bool,
    turn: Option<&TurnRow>,
    access: &dyn crate::access::ResourceAccess,
) -> Result<(), InputError> {
    let now = Time::new(unit.now());
    let principal = Principal::load(&mut *unit.conn, caller.clone(), now).await?;
    let decision = nodes::check(
        &mut *unit.conn,
        &principal,
        thread.node_id,
        Permission::ThreadInteract,
        Act::Use,
        orch_authz::Policy::default(),
    )
    .await
    .map_err(|error| match error {
        orch_authz::Error::Suspended { .. } => InputError::Denied(Box::new(Refusal::of(
            Permission::ThreadInteract,
            ObjectRef {
                kind: ObjectKind::Thread,
                id: thread.node_id,
            },
            Vec::new(),
        ))),
        other => other.into(),
    })?;
    // A refusal is a refusal whether or not it says what is missing.
    let mut refused = false;
    let mut missing = Vec::new();
    if let Decision::Refused(refusal) = decision {
        refused = true;
        missing = refusal.missing;
    }
    let provider = if let Some(turn) = turn {
        let segment = threads::segment(&mut *unit.conn, turn.segment_id)
            .await?
            .filter(|segment| segment.thread_id == thread.node_id)
            .ok_or_else(|| EngineError::unreadable("the running turn's provider is unavailable"))?;
        Some(segment.provider_id)
    } else {
        thread.provider_id
    };
    if need_provider && let Some(provider) = provider {
        // The resource supplier asks about the person, model and effort. Preserve
        // this caller's token narrowing before crossing that person-only boundary.
        if !caller.permits(Permission::ProviderUse) {
            return Err(InputError::Denied(Box::new(Refusal::of(
                Permission::ProviderUse,
                ObjectRef {
                    kind: ObjectKind::Provider,
                    id: provider,
                },
                Vec::new(),
            ))));
        }
        let settings = if turn.is_none() {
            Some(crate::resolved::load(&mut *unit.conn, thread.node_id).await?)
        } else {
            None
        };
        let answer = access
            .provider_use(
                &mut *unit.conn,
                &ProviderUse {
                    person: caller.user(),
                    node: thread.node_id,
                    provider,
                    model: turn.map(|turn| turn.model.as_str()).or_else(|| {
                        settings
                            .as_ref()
                            .and_then(|settings| settings.model.as_deref())
                    }),
                    effort: turn.map_or_else(
                        || {
                            settings
                                .as_ref()
                                .and_then(|settings| settings.effort.as_deref())
                        },
                        |turn| turn.effort.as_deref(),
                    ),
                },
            )
            .await
            .map_err(EngineError::from)?;
        if let crate::access::Answer::Refused(lacking) = answer {
            refused = true;
            missing.extend(lacking);
        }
    }
    if !refused {
        return Ok(());
    }
    Err(InputError::Denied(Box::new(Refusal::of(
        Permission::ThreadInteract,
        ObjectRef {
            kind: ObjectKind::Thread,
            id: thread.node_id,
        },
        missing,
    ))))
}

impl Engine {
    pub(crate) async fn check_content(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        content: &[ContentPart],
    ) -> Result<(), InputError> {
        self.check_content_from(unit, caller, content, None).await
    }

    /// A scope-admitted source is a trusted operand, not authority: the ordinary check revalidates
    /// its association and current readability here rather than switching to an unbound source.
    async fn check_content_from(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        content: &[ContentPart],
        selected: Option<&[(Hash, BlobOrigin)]>,
    ) -> Result<(), InputError> {
        let hashes = orch_chat::following::content_blobs(content);
        if hashes.is_empty() {
            return Ok(());
        }
        let principal = Principal::load(unit.conn, caller.clone(), self.inner.clock.time()).await?;
        if let Some(selected) = selected {
            for hash in &hashes {
                let allowed: Vec<_> = selected
                    .iter()
                    .filter_map(|(selected_hash, origin)| {
                        (selected_hash == hash).then_some(*origin)
                    })
                    .collect();
                if orch_chat::following::blob_origin_in(unit.conn, &principal, hash, &allowed)
                    .await
                    .map_err(|error| match error {
                        orch_chat::Error::NotFound => InputError::NotFound,
                        other => InputError::Internal(EngineError::unreadable(other)),
                    })?
                    .is_none()
                {
                    return Err(InputError::NotFound);
                }
            }
            return Ok(());
        }
        orch_chat::following::require_blobs(unit.conn, &principal, &hashes)
            .await
            .map_err(|error| match error {
                orch_chat::Error::NotFound => InputError::NotFound,
                other => InputError::Internal(EngineError::unreadable(other)),
            })
    }

    /// What the author of a new input needs, as the place says for one who may not use the thread's model or its
    /// environment: they drive the thread, always; without those uses their input is refused only where the place
    /// refuses it, and otherwise it is taken and served by somebody who may (`thread.inputs_without_use`).
    pub(crate) async fn check_author(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        thread: &ThreadRow,
    ) -> Result<Serving, InputError> {
        self.check_author_for_turn(unit, caller, thread, None).await
    }

    async fn check_author_for_turn(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        thread: &ThreadRow,
        turn: Option<&TurnRow>,
    ) -> Result<Serving, InputError> {
        let refusal = match self
            .check_sender_for_turn(unit, caller, thread, true, turn)
            .await
        {
            Ok(()) => return Ok(Serving::Own),
            Err(InputError::Denied(refusal)) => refusal,
            Err(other) => return Err(other),
        };
        // Boxed: a message is taken deep in a request's stack.
        let policy = Box::pin(orch_settings::get::<String>(
            &mut *unit.conn,
            orch_settings::Place::Node(thread.node_id),
            orch_settings_contract::registry::keys::THREAD_INPUTS_WITHOUT_USE,
        ))
        .await
        .map_err(EngineError::from)?
        .unwrap_or_else(|| crate::preflight::WITHOUT_MODEL_HELD.to_owned());
        // Where the place refuses such messages, the refusal names everything missing, the model's use included.
        if policy == crate::preflight::WITHOUT_MODEL_REFUSED {
            return Err(InputError::Denied(refusal));
        }
        // Whoever may not drive the thread is refused for that, whatever the model.
        self.check_sender(unit, caller, thread, false).await?;
        Ok(Serving::WithoutModel {
            held: policy != crate::preflight::WITHOUT_MODEL_ALLOWED,
        })
    }

    async fn check_authors_for_turn(
        &self,
        unit: &mut Unit<'_>,
        callers: &InputCallers,
        thread: &ThreadRow,
        turn: Option<&TurnRow>,
    ) -> Result<Serving, InputError> {
        let mut result = Serving::Own;
        for caller in callers.callers() {
            if let Serving::WithoutModel { held } = self
                .check_author_for_turn(unit, caller, thread, turn)
                .await?
            {
                result = Serving::WithoutModel {
                    held: held || matches!(result, Serving::WithoutModel { held: true }),
                };
            }
        }
        Ok(result)
    }

    /// Model approval never replaces the original author's current permission to drive.
    pub(crate) async fn check_input_author_driving(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        input: &InputRow,
    ) -> Result<(), InputError> {
        let denied = || {
            InputError::Denied(Box::new(Refusal::of(
                Permission::ThreadInteract,
                ObjectRef {
                    kind: ObjectKind::Thread,
                    id: thread.node_id,
                },
                Vec::new(),
            )))
        };
        let Some(author) = input.author_id else {
            return if input.is_authorless() {
                Ok(())
            } else {
                Err(denied())
            };
        };
        let original =
            match input_author_caller(unit, thread.node_id, input, self.inner.clock.as_ref())
                .await?
            {
                Some(original) => original,
                // Unknown historical provenance grants no model use. Its author's current
                // ordinary driving rights still bind an explicitly approved input.
                None if input.submission_authority.is_none() => {
                    InputCallers::single(Caller::session(author).with_admin_mode(false))?
                }
                None => return Err(denied()),
            };
        for caller in original.callers() {
            self.check_sender(unit, caller, thread, false).await?;
        }
        Ok(())
    }

    /// What the sender of an input needs, checked and told whole: they drive
    /// the thread, and they may use its provider with its model and effort and
    /// the environment it runs in (asked with the provider, in one question).
    pub(crate) async fn check_sender(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        thread: &ThreadRow,
        need_provider: bool,
    ) -> Result<(), InputError> {
        self.check_sender_for_turn(unit, caller, thread, need_provider, None)
            .await
    }

    pub(crate) async fn check_sender_for_turn(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        thread: &ThreadRow,
        need_provider: bool,
        turn: Option<&TurnRow>,
    ) -> Result<(), InputError> {
        check_sender_for_turn(
            unit,
            caller,
            thread,
            need_provider,
            turn,
            self.inner.access.as_ref(),
        )
        .await
    }

    /// Takes an input from a person: queued, steered into the turn that
    /// runs, or placed first with the turn stopped, as its delivery says.
    pub async fn submit_input(
        &self,
        caller: &Caller,
        thread_id: Id,
        request: InputRequest,
    ) -> Result<Submitted, InputError> {
        let tx = self.inner.db.begin().await?;
        self.submit_input_with(tx, caller, self.inner.clock.now(), thread_id, request, None)
            .await
    }

    /// As [`Engine::submit_input`], in the transaction of the API's action guard: `principal` was loaded in
    /// `tx` by that check once the request had come, and its caller and its time are the ones that take it.
    pub async fn submit_input_guarded(
        &self,
        tx: orch_db::Tx,
        principal: Principal,
        thread_id: Id,
        request: InputRequest,
    ) -> Result<Submitted, InputError> {
        let caller = principal.caller().clone();
        self.submit_input_with(tx, &caller, principal.now().get(), thread_id, request, None)
            .await
    }

    /// Accepts an explicit native selection through the same authored input and action guard.
    pub async fn submit_selected_input_guarded(
        &self,
        tx: orch_db::Tx,
        principal: Principal,
        thread: Id,
        request: InputRequest,
        selection: Option<orch_api_contract::input_schedules::NativeInvocation>,
    ) -> Result<Submitted, InputError> {
        let caller = principal.caller().clone();
        self.submit_input_with(
            tx,
            &caller,
            principal.now().get(),
            thread,
            request,
            selection,
        )
        .await
    }

    /// The same selected-input writer and commit path, with optional full response projection.
    /// `receipt_only` comes from the API's admitted operation, never from request data. The caller
    /// may narrow the response further after this method's guarded commit.
    #[allow(
        clippy::too_many_arguments,
        reason = "one existing guarded input writer carries its exact request and trusted source/projection operands"
    )]
    pub async fn submit_selected_input_projected_guarded(
        &self,
        tx: orch_db::Tx,
        principal: Principal,
        thread: Id,
        request: InputRequest,
        selection: Option<orch_api_contract::input_schedules::NativeInvocation>,
        receipt_only: bool,
        operands: Option<&InputSourceOperands>,
    ) -> Result<SubmittedProjection, InputError> {
        let caller = principal.caller().clone();
        self.submit_input_projected_with(
            tx,
            &caller,
            principal.now().get(),
            thread,
            request,
            selection,
            receipt_only,
            operands,
        )
        .await
    }

    /// Queues ordinary authored work inside the transaction which also owns its schedule occurrence.
    /// The caller commits the transaction and kicks the engine afterwards; no prior held queue is resumed.
    pub async fn submit_scheduled_input(
        &self,
        tx: &mut orch_db::Conn,
        caller: &Caller,
        thread: Id,
        request: &InputRequest,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Submitted, InputError> {
        self.submit_selected_scheduled_input(tx, caller, thread, request, now, None)
            .await
    }

    /// A scheduled selection remains one ordinary queued input under the author's current authority.
    pub async fn submit_selected_scheduled_input(
        &self,
        tx: &mut orch_db::Conn,
        caller: &Caller,
        thread: Id,
        request: &InputRequest,
        now: chrono::DateTime<chrono::Utc>,
        selection: Option<&orch_api_contract::input_schedules::NativeInvocation>,
    ) -> Result<Submitted, InputError> {
        if request.delivery != Delivery::Queue {
            return Err(InputError::Invalid(
                "a scheduled input uses the ordinary queue",
            ));
        }
        let mut unit = Unit::new(tx, now);
        let mut taken = self
            .submit_selected_in(&mut unit, caller, thread, request, selection)
            .await?;
        let finished = unit.finish().await?;
        if taken.submitted.created {
            taken.submitted.seq = finished
                .touched
                .get(&thread)
                .copied()
                .unwrap_or(taken.submitted.seq);
        }
        Ok(taken.submitted)
    }

    /// Accept a real scheduled occurrence using every captured author-definition origin. The
    /// controller who merely resumed a schedule is not substituted for its execution author.
    #[allow(
        clippy::too_many_arguments,
        reason = "one ordinary scheduled producer retains typed origin and exact accepted definition boundaries"
    )]
    pub async fn submit_selected_scheduled_input_from_origins(
        &self,
        conn: &mut orch_db::Conn,
        author: Id,
        origins: &[orch_authz::CapturedOperation],
        reference: ScheduledInputOrigin,
        thread: Id,
        request: &InputRequest,
        now: chrono::DateTime<chrono::Utc>,
        selection: Option<&orch_api_contract::input_schedules::NativeInvocation>,
    ) -> Result<Submitted, InputError> {
        if request.delivery != Delivery::Queue {
            return Err(InputError::Invalid(
                "a scheduled input uses the ordinary queue",
            ));
        }
        let authority = InputAuthority::for_schedule(author, origins, reference)?;
        let mut unit = Unit::new(conn, now);
        let schedule =
            orch_db::queries::input_schedules::schedule(unit.conn, reference.schedule_id, false)
                .await?
                .filter(|row| row.author_id == author)
                .ok_or(InputError::NotFound)?;
        let run = orch_db::queries::input_schedules::run_by_id(unit.conn, reference.run_id)
            .await?
            .filter(|run| {
                run.schedule_id == schedule.id
                    && run.thread_id.is_none_or(|actual| actual == thread)
            })
            .ok_or(InputError::NotFound)?;
        if reference.destination == ScheduledInputDestination::NewThread
            && thread
                != crate::ids::derived(
                    "input-schedule.thread",
                    &[schedule.id.as_bytes(), run.occurrence.as_bytes()],
                )
        {
            return Err(InputError::NotFound);
        }
        let row = threads::lock(unit.conn, thread)
            .await?
            .ok_or(InputError::NotFound)?;
        let callers = input_authority_for_request(
            &mut unit,
            &row,
            request,
            selection,
            author,
            &authority,
            self.inner.clock.as_ref(),
        )
        .await?
        .ok_or_else(|| {
            InputError::Denied(Box::new(Refusal::of(
                Permission::ThreadInteract,
                ObjectRef {
                    kind: ObjectKind::Thread,
                    id: thread,
                },
                Vec::new(),
            )))
        })?;
        let mut taken = self
            .submit_with_authority_in(&mut unit, &callers, thread, request, selection, Made::USER)
            .await?;
        let finished = unit.finish().await?;
        if taken.submitted.created {
            taken.submitted.seq = finished
                .touched
                .get(&thread)
                .copied()
                .unwrap_or(taken.submitted.seq);
        }
        Ok(taken.submitted)
    }

    async fn submit_input_with(
        &self,
        tx: orch_db::Tx,
        caller: &Caller,
        now: chrono::DateTime<chrono::Utc>,
        thread_id: Id,
        request: InputRequest,
        selection: Option<orch_api_contract::input_schedules::NativeInvocation>,
    ) -> Result<Submitted, InputError> {
        self.submit_input_projected_with(
            tx, caller, now, thread_id, request, selection, false, None,
        )
        .await?
        .full()
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the existing authored request and trusted response projection stay separate on one writer"
    )]
    #[allow(
        clippy::too_many_arguments,
        reason = "one existing guarded input writer carries its exact request and trusted source/projection operands"
    )]
    async fn submit_input_projected_with(
        &self,
        mut tx: orch_db::Tx,
        caller: &Caller,
        now: chrono::DateTime<chrono::Utc>,
        thread_id: Id,
        request: InputRequest,
        selection: Option<orch_api_contract::input_schedules::NativeInvocation>,
        receipt_only: bool,
        operands: Option<&InputSourceOperands>,
    ) -> Result<SubmittedProjection, InputError> {
        let (finished, submitted, interrupt) = {
            let mut unit = Unit::new(&mut tx, now);
            let taken = self
                .submit_selected_made_row_in(
                    &mut unit,
                    InputExecution::Direct(caller),
                    thread_id,
                    &request,
                    selection.as_ref(),
                    Made::USER,
                    operands,
                    None,
                )
                .await?;
            let (mut submitted, interrupt) = taken.project(&mut *unit.conn, receipt_only).await?;
            let finished = unit.finish().await?;
            if submitted.created {
                submitted.seq = finished
                    .touched
                    .get(&thread_id)
                    .copied()
                    .unwrap_or(submitted.seq);
            }
            (finished, submitted, interrupt)
        };
        self.commit(tx, finished).await?;
        if interrupt.is_some() {
            self.drive_interrupt(thread_id).await?;
        }
        Ok(submitted)
    }

    /// Takes an input inside a unit of work whose transaction its owner holds and commits: the same checks and the
    /// same rows as [`Engine::submit_input`], so that an input can be one of several things that are written
    /// together, as a forward is. After the commit the owner takes up what the unit asked for, and for an input
    /// that interrupted a turn drives the interrupt.
    pub(crate) async fn submit_input_in(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        thread_id: Id,
        request: &InputRequest,
    ) -> Result<Taken, InputError> {
        self.submit_selected_in(unit, caller, thread_id, request, None)
            .await
    }

    async fn submit_selected_in(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        thread_id: Id,
        request: &InputRequest,
        selection: Option<&orch_api_contract::input_schedules::NativeInvocation>,
    ) -> Result<Taken, InputError> {
        self.submit_selected_made_in(unit, caller, thread_id, request, selection, Made::USER)
            .await
    }

    pub(crate) async fn submit_with_authority_in(
        &self,
        unit: &mut Unit<'_>,
        callers: &InputCallers,
        thread_id: Id,
        request: &InputRequest,
        selection: Option<&orch_api_contract::input_schedules::NativeInvocation>,
        made: Made<'_>,
    ) -> Result<Taken, InputError> {
        let taken = self
            .submit_selected_made_row_in(
                unit,
                InputExecution::Origins(callers),
                thread_id,
                request,
                selection,
                made,
                None,
                None,
            )
            .await?;
        let (submitted, interrupt) = taken.project(unit.conn, false).await?;
        Ok(Taken {
            submitted: submitted.full()?,
            interrupt,
        })
    }

    pub(crate) async fn submit_chat_message_in(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        thread_id: Id,
        request: &InputRequest,
        reference: ChatInputOrigin,
    ) -> Result<Taken, InputError> {
        if reference.thread_id != thread_id {
            return Err(InputError::Invalid(
                "the agent ask has a different recipient",
            ));
        }
        let authority = InputAuthority::for_chat(caller, reference)?;
        let thread = threads::lock(unit.conn, thread_id)
            .await?
            .ok_or(InputError::NotFound)?;
        let callers = input_authority_for_request(
            unit,
            &thread,
            request,
            None,
            caller.user(),
            &authority,
            self.inner.clock.as_ref(),
        )
        .await?
        .ok_or_else(|| {
            InputError::Denied(Box::new(Refusal::of(
                Permission::ThreadInteract,
                ObjectRef {
                    kind: ObjectKind::Thread,
                    id: thread_id,
                },
                Vec::new(),
            )))
        })?;
        let taken = self
            .submit_selected_made_row_in(
                unit,
                InputExecution::Origins(&callers),
                thread_id,
                request,
                None,
                Made::USER,
                None,
                Some(reference.input_id),
            )
            .await?;
        if taken.row.id != reference.input_id {
            return Err(InputError::Invalid(
                "the accepted input has different provenance",
            ));
        }
        let (submitted, interrupt) = taken.project(unit.conn, false).await?;
        Ok(Taken {
            submitted: submitted.full()?,
            interrupt,
        })
    }

    /// The one ordinary input writer, with exact selected source witnesses from the current
    /// guarded ForwardCreate preparation. Replays keep the original accepted row/provenance.
    #[allow(
        clippy::too_many_arguments,
        reason = "one shared writer receives trusted origin, source and projection facts"
    )]
    pub(crate) async fn submit_forward_create_in(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        thread: Id,
        request: &InputRequest,
        acceptance: &ForwardAcceptance,
        receipt_only: bool,
        selected: &[(Hash, orch_chat::following::BlobOrigin)],
    ) -> Result<SubmittedProjection, InputError> {
        let mut operands = InputSourceOperands {
            blobs: selected.to_vec(),
            replayed: None,
            bypass_subjects: Vec::new(),
            native_permission: None,
        };
        let (bundle, allocated) = match acceptance {
            ForwardAcceptance::New(reference) => {
                if reference.thread_id != thread {
                    return Err(InputError::Invalid("the forward has a different recipient"));
                }
                // Recheck the concrete receiver through the shared restriction predicate; the
                // complete source proof and selected witnesses came from the ordinary HTTP guard.
                let target = crate::operation_scope::targets::node(unit.conn, thread)
                    .await
                    .map_err(EngineError::unreadable)?;
                let plan = crate::operation_scope::Prepared::for_selection("ForwardCreate")
                    .map_err(EngineError::unreadable)?;
                let index = plan
                    .variant()
                    .facets
                    .iter()
                    .position(|facet| {
                        facet.target
                            == orch_api_contract::operations::TargetResolver::ResolvedCollection(
                                "forward_destination_threads",
                            )
                    })
                    .ok_or_else(|| {
                        EngineError::unreadable("the forward recipient collection is not declared")
                    })?;
                if !plan
                    .permits_binding(
                        caller,
                        index,
                        orch_access_contract::key_scope::KeyTargetKind::Object(ObjectKind::Thread),
                        &[target],
                    )
                    .map_err(EngineError::unreadable)?
                {
                    return Err(InputError::Denied(Box::new(Refusal::of(
                        Permission::ThreadInteract,
                        ObjectRef {
                            kind: ObjectKind::Thread,
                            id: thread,
                        },
                        Vec::new(),
                    ))));
                }
                (
                    Some(InputCallers::accepted_forward(
                        caller.clone(),
                        reference.clone(),
                    )?),
                    Some(reference.input_id),
                )
            }
            ForwardAcceptance::Replay(id) => {
                operands.replayed = Some(*id);
                (None, None)
            }
        };
        let execution = bundle
            .as_ref()
            .map_or(InputExecution::Direct(caller), InputExecution::Origins);
        let taken = self
            .submit_selected_made_row_in(
                unit,
                execution,
                thread,
                request,
                None,
                Made::USER,
                Some(&operands),
                allocated,
            )
            .await?;
        if allocated.is_some_and(|id| taken.row.id != id) {
            return Err(orch_db::DbError::Retry.into());
        }
        let (submitted, interrupt) = taken.project(unit.conn, receipt_only).await?;
        debug_assert!(
            interrupt.is_none(),
            "a forward uses ordinary queued delivery"
        );
        Ok(submitted)
    }

    /// Accept only the separately requested comment after its actual copy row exists. Replay
    /// keeps the prior input and authority; the comment never becomes a copy-delivery permission.
    pub(crate) async fn submit_snapshot_comment_in(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        copy: &orch_db::queries::shares::CopyRow,
        request: &InputRequest,
    ) -> Result<Id, InputError> {
        if copy.created_by != caller.user() {
            return Err(InputError::NotTheAuthor);
        }
        let prior = threads::input_by_client(
            unit.conn,
            copy.thread_id,
            Some(caller.user()),
            &request.client_id,
        )
        .await?;
        let replayed = prior.as_ref().map(|input| input.id);
        let (bundle, allocated) = if prior.is_some() {
            // The same ordinary idempotency rule returns the original acceptance, without
            // assigning this caller/copy as new provenance for an unrelated earlier input.
            (None, None)
        } else {
            let reference = SnapshotCommentOrigin {
                copy_id: copy.id,
                source: SnapshotCommentOrigin::source_of(copy)?,
                thread_id: copy.thread_id,
                input_id: new_id(),
            };
            if caller
                .grant()
                .and_then(|grant| grant.operation_scope())
                .is_some()
            {
                let target = crate::operation_scope::targets::node(unit.conn, copy.thread_id)
                    .await
                    .map_err(EngineError::unreadable)?;
                let plan = crate::operation_scope::Prepared::for_selection(
                    "ShareAgentCreate.snapshot_comment",
                )
                .map_err(EngineError::unreadable)?;
                let index = plan
                    .variant()
                    .facets
                    .iter()
                    .position(|facet| {
                        facet.target
                            == orch_api_contract::operations::TargetResolver::ResolvedCollection(
                                "snapshot_recipient_threads",
                            )
                    })
                    .ok_or_else(|| {
                        EngineError::unreadable("the snapshot comment recipient is not declared")
                    })?;
                if !plan
                    .permits_binding(
                        caller,
                        index,
                        orch_access_contract::key_scope::KeyTargetKind::Object(ObjectKind::Thread),
                        &[target],
                    )
                    .map_err(EngineError::unreadable)?
                {
                    return Err(InputError::Denied(Box::new(Refusal::of(
                        Permission::ThreadInteract,
                        ObjectRef {
                            kind: ObjectKind::Thread,
                            id: copy.thread_id,
                        },
                        Vec::new(),
                    ))));
                }
            }
            (
                Some(InputCallers::accepted_snapshot_comment(
                    caller.clone(),
                    reference,
                )?),
                Some(reference.input_id),
            )
        };
        let execution = bundle
            .as_ref()
            .map_or(InputExecution::Direct(caller), InputExecution::Origins);
        let operands = InputSourceOperands {
            blobs: Vec::new(),
            replayed,
            bypass_subjects: Vec::new(),
            native_permission: None,
        };
        let taken = self
            .submit_selected_made_row_in(
                unit,
                execution,
                copy.thread_id,
                request,
                None,
                Made::USER,
                Some(&operands),
                allocated,
            )
            .await?;
        if allocated.is_some_and(|id| taken.row.id != id) {
            return Err(orch_db::DbError::Retry.into());
        }
        let (submitted, interrupt) = taken.project(unit.conn, true).await?;
        debug_assert!(
            interrupt.is_none(),
            "a snapshot comment uses ordinary queued delivery"
        );
        Ok(submitted.input_id)
    }

    async fn submit_selected_made_in(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        thread_id: Id,
        request: &InputRequest,
        selection: Option<&orch_api_contract::input_schedules::NativeInvocation>,
        made: Made<'_>,
    ) -> Result<Taken, InputError> {
        let taken = self
            .submit_selected_made_row_in(
                unit,
                InputExecution::Direct(caller),
                thread_id,
                request,
                selection,
                made,
                None,
                None,
            )
            .await?;
        let (submitted, interrupt) = taken.project(&mut *unit.conn, false).await?;
        Ok(Taken {
            submitted: submitted.full()?,
            interrupt,
        })
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the shared input preparation retains actual current operands and authority"
    )]
    async fn prepare_input_work(
        &self,
        unit: &mut Unit<'_>,
        execution: &InputExecution<'_>,
        thread: &ThreadRow,
        request: &InputRequest,
        selection: Option<&orch_api_contract::input_schedules::NativeInvocation>,
        operands: Option<&InputSourceOperands>,
        steered_turn: Option<&TurnRow>,
    ) -> Result<PreparedInputWork, InputError> {
        let thread_id = thread.node_id;
        // Boxed: a message is taken deep in a request's stack. A steer uses
        // the running turn's recorded provider, model and effort.
        let mut serving = Serving::Own;
        for caller in execution.callers() {
            if let Serving::WithoutModel { held } =
                Box::pin(self.check_author_for_turn(unit, caller, thread, steered_turn)).await?
            {
                serving = Serving::WithoutModel {
                    held: held || matches!(serving, Serving::WithoutModel { held: true }),
                };
            }
        }
        if request.plan && !Box::pin(self.agent_plans(unit, thread)).await? {
            return Err(InputError::PlanUnsupported);
        }
        let mut verified = None;
        for caller in execution.callers() {
            self.check_content_from(
                unit,
                caller,
                &request.content,
                operands.map(|prepared| prepared.blobs.as_slice()),
            )
            .await?;
            if let Some(selection) = selection {
                verified = Some(
                    crate::native_actions::validate_selected(
                        unit,
                        caller,
                        thread_id,
                        selection,
                        request.delivery,
                    )
                    .await?,
                );
            }
        }
        if operands.is_some_and(|prepared| {
            prepared.native_permission != verified.as_ref().map(|action| action.permission)
        }) {
            return Err(orch_db::DbError::Retry.into());
        }
        if let Some(selection) = selection {
            let bytes = serde_json::to_vec(&request.content)
                .map_err(EngineError::from)?
                .len()
                + serde_json::to_vec(selection)
                    .map_err(EngineError::from)?
                    .len();
            if bytes > orch_api_contract::common::MAX_JSON_BYTES {
                return Err(InputError::TooLong);
            }
        }
        let mut bypass = None;
        for caller in execution.callers() {
            let checked = self
                .bypass_of_from(
                    unit,
                    caller,
                    thread,
                    Some(execution.user()),
                    request.bypass.as_ref(),
                    operands.map(|prepared| prepared.bypass_subjects.as_slice()),
                )
                .await?;
            if bypass.is_none() {
                bypass = checked;
            }
        }
        if !execution.callers().iter().all(Caller::as_admin) {
            if let Some(entries) = bypass.as_mut().and_then(Json::as_array_mut) {
                for entry in entries {
                    if let Some(entry) = entry.as_object_mut() {
                        entry.remove("as_admin");
                    }
                }
            }
        }
        let mut prepared = self
            .prepare_pastes(unit.conn, thread, &request.content)
            .await?;
        self.prepare_attachment_names(unit.conn, thread_id, &mut prepared)
            .await?;
        Ok(PreparedInputWork {
            serving,
            verified,
            bypass,
            content: prepared,
        })
    }

    /// The same input writer's ordinary validation/materialization, without queue rows or captured
    /// execution authority. Schedule creation/resume invokes it inside its existing rollback trial.
    #[allow(
        clippy::too_many_arguments,
        reason = "ordinary schedule validation retains the exact selected source operands without manufacturing an execution origin"
    )]
    pub async fn validate_selected_scheduled_input(
        &self,
        conn: &mut orch_db::Conn,
        caller: &Caller,
        thread: Id,
        request: &InputRequest,
        now: chrono::DateTime<chrono::Utc>,
        selection: Option<&orch_api_contract::input_schedules::NativeInvocation>,
        operands: Option<&InputSourceOperands>,
    ) -> Result<(), InputError> {
        if request.delivery != Delivery::Queue {
            return Err(InputError::Invalid(
                "a scheduled input uses the ordinary queue",
            ));
        }
        validate_request(request, selection)?;
        let mut unit = Unit::new(conn, now);
        let thread = threads::lock(unit.conn, thread)
            .await?
            .ok_or(InputError::NotFound)?;
        if thread.deleted_at.is_some() {
            return Err(InputError::NotFound);
        }
        if thread.archived_at.is_some() {
            return Err(InputError::Archived);
        }
        self.prepare_input_work(
            &mut unit,
            &InputExecution::Direct(caller),
            &thread,
            request,
            selection,
            operands,
            None,
        )
        .await?;
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one existing guarded input writer carries its exact request and trusted source/projection operands"
    )]
    async fn submit_selected_made_row_in(
        &self,
        unit: &mut Unit<'_>,
        execution: InputExecution<'_>,
        thread_id: Id,
        request: &InputRequest,
        selection: Option<&orch_api_contract::input_schedules::NativeInvocation>,
        made: Made<'_>,
        operands: Option<&InputSourceOperands>,
        allocated_input: Option<Id>,
    ) -> Result<TakenRow, InputError> {
        validate_request(request, selection)?;
        let now = unit.now();
        let mut interrupt_turn: Option<Id> = None;
        let thread = threads::lock(&mut *unit.conn, thread_id)
            .await?
            .ok_or(InputError::NotFound)?;
        if thread.deleted_at.is_some() {
            return Err(InputError::NotFound);
        }
        if thread.archived_at.is_some() {
            return Err(InputError::Archived);
        }
        // A repeat returns what stands, before anything is asked again.
        if let Some(found) = threads::input_by_client(
            &mut *unit.conn,
            thread_id,
            Some(execution.user()),
            &request.client_id,
        )
        .await?
        {
            if operands
                .and_then(|prepared| prepared.replayed)
                .is_some_and(|id| id != found.id)
            {
                return Err(orch_db::DbError::Retry.into());
            }
            return Ok(TakenRow {
                row: found,
                seq: thread.seq(),
                created: false,
                interrupt: None,
            });
        }
        if operands.and_then(|prepared| prepared.replayed).is_some() {
            return Err(orch_db::DbError::Retry.into());
        }
        let active = threads::active_turn(&mut *unit.conn, thread_id).await?;
        let queue_locked =
            thread.queue_held || crate::locks::held_by(unit.conn, thread_id).await?.is_some();
        let steered_turn = active.as_ref().filter(|turn| {
            !queue_locked
                && request.delivery == Delivery::Steer
                && turn.state == threads::turn_state::RUNNING
                && turn.plan == request.plan
        });
        let PreparedInputWork {
            serving,
            verified,
            bypass,
            content: prepared,
        } = self
            .prepare_input_work(
                unit,
                &execution,
                &thread,
                request,
                selection,
                operands,
                steered_turn,
            )
            .await?;
        let without_model = matches!(serving, Serving::WithoutModel { .. });
        let content = serde_json::to_value(&prepared).map_err(EngineError::from)?;
        // Where it goes, and in what state.
        let (state, place, turn) = if queue_locked {
            // Stop owns the queue until Resume. Neither a new send nor its delivery preference
            // may overtake the held work or steer into the turn that is still stopping.
            (input_state::PENDING, Place::End, None)
        } else {
            match (request.delivery, &active) {
                (Delivery::Queue, _) => (input_state::PENDING, Place::End, None),
                _ if without_model => (input_state::PENDING, Place::End, None),
                // Planning and working are turns of their own: words for the other kind wait for theirs.
                (Delivery::Steer, Some(turn))
                    if turn.state == threads::turn_state::RUNNING && turn.plan != request.plan =>
                {
                    (input_state::PENDING, Place::Front, None)
                }
                (Delivery::Steer, Some(turn)) if turn.state == threads::turn_state::RUNNING => {
                    if !self.agent_steers(unit, &thread).await? {
                        return Err(InputError::SteerUnsupported);
                    }
                    (input_state::DELIVERING, Place::End, Some(turn.id))
                }
                (Delivery::Steer | Delivery::Interrupt, _) => {
                    (input_state::PENDING, Place::Front, None)
                }
            }
        };
        let seq = unit.stamp(thread_id).await?;
        let input_id = allocated_input.unwrap_or_else(new_id);
        let authority = execution.captured()?;
        // The business stamp remains fixed. Natural key expiry is checked at this new
        // acceptance boundary, after all preparation and sequence-lock waits.
        if execution
            .callers()
            .iter()
            .any(|caller| !caller.key_deadline_allows(self.inner.clock.as_ref()))
        {
            return Err(InputError::Denied(Box::new(Refusal::of(
                Permission::ThreadInteract,
                ObjectRef {
                    kind: ObjectKind::Thread,
                    id: thread_id,
                },
                Vec::new(),
            ))));
        }
        let inserted = threads::insert_input(
            &mut *unit.conn,
            &NewInput {
                plan: request.plan,
                id: input_id,
                thread_id,
                author_id: Some(execution.user()),
                submission_authority: Some(&authority),
                run_as: None,
                run_as_authority: None,
                source: made.source,
                content: &content,
                delivery: delivery_word(request.delivery),
                state,
                turn_id: turn,
                client_id: Some(&request.client_id),
                limit_bypass: bypass.as_ref(),
                place,
                at: now,
            },
            seq,
        )
        .await?;
        let Some(mut row) = inserted else {
            return Err(
                EngineError::unreadable("an input that was not there could not be made").into(),
            );
        };
        if let Some((goal, facts)) = made.goal {
            orch_db::queries::goals::tie_input(&mut *unit.conn, row.id, goal, facts).await?;
        }
        if let (Some(selection), Some(verified)) = (selection, verified) {
            let raw = serde_json::to_value(selection).map_err(EngineError::from)?;
            let info =
                json!({"action_id":verified.id,"kind":verified.kind,"origin":verified.origin});
            sqlx::query(
                "UPDATE thread_inputs SET native_action=$2,native_action_info=$3 WHERE id=$1",
            )
            .bind(row.id)
            .bind(&raw)
            .bind(info)
            .execute(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
            row.native_action = Some(raw);
        }
        orch_db::queries::storage::input_pins(
            &mut *unit.conn,
            row.id,
            &orch_chat::following::content_blobs(&request.content),
            now,
        )
        .await?;
        for part in &prepared {
            let (path, blob) = match part {
                ContentPart::File {
                    path,
                    blob: Some(blob),
                    ..
                } => (path.clone(), *blob),
                ContentPart::Image { name, blob, .. } => {
                    (format!(".orch/attachments/{name}"), *blob)
                }
                ContentPart::PastedText(paste) => (paste.path.clone(), paste.blob),
                _ => continue,
            };
            sqlx::query("UPDATE thread_attachment_uploads SET blob=NULL,expires_at=NULL,created_by=NULL WHERE thread_id=$1 AND path=$2 AND blob=$3")
                .bind(thread_id).bind(path).bind(blob.as_bytes().as_slice()).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        }
        if serving == (Serving::WithoutModel { held: true })
            && threads::hold_input(&mut *unit.conn, row.id, Some(threads::HELD_MODEL_USE), seq)
                .await?
        {
            row.state_detail = Some(threads::HELD_MODEL_USE.to_owned());
            self.source_requested(
                unit,
                &thread,
                orch_db::queries::notifications::inbox::RequestSource::Input(row.id),
                None,
            )
            .await?;
        }
        // What a goal's loop says is Orch's text of the goal's own words: it follows nobody and mentions nobody.
        if made.source == "user" {
            crate::notifications::follow_thread(&mut *unit.conn, execution.user(), thread_id, now)
                .await?;
            crate::notifications::thread_mentions(&mut *unit.conn, &thread, &row, now).await?;
            // A goal that has ended leaves the line at the next person's input, so that it does not linger above new work.
            if orch_db::queries::goals::clear_ended(&mut *unit.conn, thread_id, now)
                .await?
                .is_some()
            {
                threads::set_seq_state(&mut *unit.conn, thread_id, seq).await?;
            }
        }
        let prior_plans:Vec<Id>=sqlx::query_scalar("SELECT a.object_id FROM approvals a JOIN items i ON i.id=a.object_id WHERE a.kind='plan' AND a.state='pending' AND a.thread_id=$1 AND i.continuation_id=$2").bind(thread_id).bind(thread.active_continuation_id).fetch_all(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        for plan in prior_plans {
            Box::pin(self.cancel_source_request(
                unit,
                orch_db::queries::notifications::inbox::RequestSource::Plan(plan),
                Some(execution.user()),
            ))
            .await?;
        }
        if !row.queue_held
            && !without_model
            && request.delivery == Delivery::Interrupt
            && let Some(turn) = &active
        {
            threads::set_interrupt_cause(&mut *unit.conn, turn.id, "person").await?;
            threads::mark_interrupted_by(&mut *unit.conn, turn.id, execution.user(), seq).await?;
            interrupt_turn = Some(turn.id);
        }
        // Explicit new work supersedes an attention stop or a cached quota-reset
        // wait; a goal loop does not, and ordinary non-quota backoff still binds.
        if made.source == "user" {
            crate::calls::supersede_for_input(&mut *unit.conn, thread_id).await?;
        }
        // A turn might start: the checks run in this transaction.
        if !row.queue_held
            && active.is_none()
            && matches!(
                thread.state.as_str(),
                thread_state::IDLE | thread_state::BLOCKED
            )
        {
            // Keep admission's large future out of the enclosing input state.
            let outcome = match Box::pin(self.preflight_in(unit, thread_id)).await {
                Ok(outcome) => outcome,
                Err(error) if !matches!(error, EngineError::Db(_)) => {
                    tracing::warn!(%error, %thread_id, "pre-flight could not decide; the input waits");
                    PreflightOutcome::Idle
                }
                Err(error) => return Err(error.into()),
            };
            let _ = outcome;
        }
        unit.wake(thread_id);
        let latest = threads::input(&mut *unit.conn, row.id)
            .await?
            .unwrap_or(row);
        Ok(TakenRow {
            row: latest,
            seq,
            created: true,
            interrupt: interrupt_turn,
        })
    }

    /// Queues an input that no person sent (an announcement, a notice of the
    /// system) at the end of a thread's queue. It is asked of the thread's
    /// first owner when its turn is about to start. The answer is the input.
    pub async fn queue_system_input(
        &self,
        thread_id: Id,
        source: &str,
        content: &[ContentPart],
    ) -> Result<InputRow, EngineError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let (finished, row) = {
            let mut unit = Unit::new(&mut tx, now);
            let thread = threads::lock(&mut *unit.conn, thread_id)
                .await?
                .ok_or_else(|| EngineError::unreadable("there is no such thread"))?;
            let seq = unit.stamp(thread_id).await?;
            let content = serde_json::to_value(content)?;
            let row = threads::insert_input(
                &mut *unit.conn,
                &NewInput {
                    plan: false,
                    id: new_id(),
                    thread_id,
                    author_id: None,
                    submission_authority: None,
                    run_as: None,
                    run_as_authority: None,
                    source,
                    content: &content,
                    delivery: "queue",
                    state: input_state::PENDING,
                    turn_id: None,
                    client_id: None,
                    limit_bypass: None,
                    place: Place::End,
                    at: now,
                },
                seq,
            )
            .await?
            .ok_or_else(|| EngineError::unreadable("an input could not be queued"))?;
            if !row.queue_held
                && matches!(
                    thread.state.as_str(),
                    thread_state::IDLE | thread_state::BLOCKED
                )
                && threads::active_turn(&mut *unit.conn, thread_id)
                    .await?
                    .is_none()
                && let Err(error) = self.preflight_in(&mut unit, thread_id).await
            {
                if matches!(error, EngineError::Db(_)) {
                    return Err(error);
                }
                tracing::warn!(%error, %thread_id, "pre-flight could not decide; the input waits");
            }
            unit.wake(thread_id);
            (unit.finish().await?, row)
        };
        self.commit(tx, finished).await?;
        Ok(row)
    }

    /// Whether the thread's agent takes a message into a turn that runs.
    async fn agent_steers(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
    ) -> Result<bool, EngineError> {
        Ok(self
            .agent_capabilities(unit, thread)
            .await?
            .is_some_and(|capabilities| capabilities.steer))
    }

    /// Whether the thread's agent program can work a turn in plan mode.
    async fn agent_plans(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
    ) -> Result<bool, EngineError> {
        Ok(self
            .agent_capabilities(unit, thread)
            .await?
            .is_some_and(|capabilities| capabilities.plan_mode))
    }

    /// What the thread's agent program declares it can do; none where its profile never reported.
    async fn agent_capabilities(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
    ) -> Result<Option<orch_agent_contract::descriptor::Capabilities>, EngineError> {
        let Some(agent) = thread.agent_id else {
            return Ok(None);
        };
        let Some(agent) =
            orch_db::queries::resources::thread_engine::agent(&mut *unit.conn, agent).await?
        else {
            return Ok(None);
        };
        Ok(crate::harnesses::descriptor_of(&agent)?.map(|descriptor| descriptor.capabilities))
    }

    /// The limits an input is let past, as the record the row keeps; the sender must hold what each needs.
    /// A cap is named by its identifier: one that does not stand in the way of this author's turn, or has
    /// room, is left out without an error; one the sender does not hold the row for refuses the request.
    /// An input that no person sent is never let past a limit.
    #[allow(
        clippy::too_many_arguments,
        reason = "the ordinary bypass producer rechecks trusted current operation homes"
    )]
    async fn bypass_of_from(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        thread: &ThreadRow,
        author: Option<Id>,
        asked: Option<&LimitBypassRequest>,
        selected: Option<&[orch_authz::caps::Home]>,
    ) -> Result<Option<Json>, InputError> {
        let Some(asked) = asked else {
            return Ok(None);
        };
        let named: Vec<Id> = asked.caps.clone().unwrap_or_default();
        // Legacy vendor-limit flags are accepted but have no effect. Only named
        // configured Orch caps can create a bypass action.
        if named.is_empty() {
            return Ok(None);
        }
        let Some(author) = author else {
            return Err(InputError::Invalid(
                "an input that no person sent is never let past a limit",
            ));
        };
        let caps = self
            .caps_to_pass(unit, caller, thread, author, &named, selected)
            .await?;
        if caps.is_empty() {
            return Ok(None);
        }
        let mut entry = json!({
            "by": caller.user(), "at": views::time_json(self.inner.clock.now()), "caps": caps,
            "provider_limit": false,
        });
        // Whether the giver acted with As admin on: a system admin passes a cap of the system only so,
        // and is asked again, as they acted, before the turn starts.
        if caller.as_admin() {
            entry["as_admin"] = json!(true);
        }
        Ok(Some(json!([entry])))
    }

    /// Of the caps named, those that stand in the way of the turn of `author` in this thread and that the
    /// caller may let it past.
    /// Actual homes affected by an explicit input bypass, using the same applicable/reached
    /// cap selection as the writer. Facts only: normal bypass permission checks remain in place.
    pub async fn input_bypass_subjects(
        &self,
        conn: &mut orch_db::Conn,
        thread: &ThreadRow,
        author: Id,
        asked: &LimitBypassRequest,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<orch_authz::caps::Home>, InputError> {
        let mut unit = Unit::new(conn, now);
        let named = asked.caps.as_deref().unwrap_or_default();
        let homes: Vec<_> = self
            .input_caps_to_pass(&mut unit, thread, author, named)
            .await?
            .iter()
            .map(crate::caps::home_of)
            .collect();
        Ok(homes)
    }

    async fn input_caps_to_pass(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        author: Id,
        named: &[Id],
    ) -> Result<Vec<orch_db::usage::caps::CapRow>, InputError> {
        operation::caps_to_pass(unit, thread, author, named).await
    }

    async fn caps_to_pass(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        thread: &ThreadRow,
        author: Id,
        named: &[Id],
        selected: Option<&[orch_authz::caps::Home]>,
    ) -> Result<Vec<Id>, InputError> {
        if named.is_empty() {
            return Ok(Vec::new());
        }
        let reached = self.input_caps_to_pass(unit, thread, author, named).await?;
        let now = Time::new(unit.now());
        let principal = Principal::load(unit.conn, caller.clone(), now).await?;
        let mut passes = Vec::with_capacity(reached.len());
        for cap in reached {
            if selected
                .is_some_and(|selected| !bypass_was_prepared(selected, crate::caps::home_of(&cap)))
            {
                return Err(orch_db::DbError::Retry.into());
            }
            match orch_authz::caps::check_pass(
                unit.conn,
                &principal,
                crate::caps::home_of(&cap),
                orch_authz::Policy::default(),
            )
            .await?
            {
                orch_authz::caps::Verdict::Allowed => passes.push(cap.id),
                orch_authz::caps::Verdict::Refused(refusal) => {
                    return Err(InputError::Denied(refusal));
                }
                orch_authz::caps::Verdict::SystemAdminOnly => {
                    return Err(InputError::Invalid(
                        "only a system admin lets a turn past a cap of the system",
                    ));
                }
            }
        }
        Ok(passes)
    }

    /// Changes an input that waits: its words (by its author alone), and its
    /// place (by anyone who may drive the thread): to the front when
    /// `after` is `Some(None)`, directly after another input that waits when
    /// it names one.
    pub async fn edit_input(
        &self,
        caller: &Caller,
        thread_id: Id,
        input_id: Id,
        content: Option<Vec<ContentPart>>,
        after: Option<Option<Id>>,
    ) -> Result<ThreadInput, InputError> {
        let tx = self.inner.db.begin().await?;
        let now = self.inner.clock.now();
        self.edit_input_with(
            tx,
            caller,
            now,
            thread_id,
            input_id,
            ThreadInputUpdateBody {
                content,
                after_input_id: after,
                ..ThreadInputUpdateBody::default()
            },
        )
        .await
        .map(|changed| changed.input)
    }

    /// As [`Engine::edit_input`], in the transaction of the API's action guard, with its caller and its time.
    pub async fn edit_input_guarded(
        &self,
        tx: orch_db::Tx,
        principal: Principal,
        thread_id: Id,
        input_id: Id,
        content: Option<Vec<ContentPart>>,
        after: Option<Option<Id>>,
    ) -> Result<ThreadInput, InputError> {
        let caller = principal.caller().clone();
        let now = principal.now().get();
        self.edit_input_with(
            tx,
            &caller,
            now,
            thread_id,
            input_id,
            ThreadInputUpdateBody {
                content,
                after_input_id: after,
                ..ThreadInputUpdateBody::default()
            },
        )
        .await
        .map(|changed| changed.input)
    }

    /// Changes a pending input under the API's action guard. Its author changes its
    /// words or native command; anyone who drives may change its delivery or place.
    pub async fn update_input_guarded(
        &self,
        tx: orch_db::Tx,
        principal: Principal,
        thread: Id,
        input: Id,
        update: ThreadInputUpdateBody,
    ) -> Result<ThreadInputUpdateReply, InputError> {
        let caller = principal.caller().clone();
        self.edit_input_with(tx, &caller, principal.now().get(), thread, input, update)
            .await
    }

    async fn edit_input_with(
        &self,
        mut tx: orch_db::Tx,
        caller: &Caller,
        now: chrono::DateTime<chrono::Utc>,
        thread_id: Id,
        input_id: Id,
        update: ThreadInputUpdateBody,
    ) -> Result<ThreadInputUpdateReply, InputError> {
        let ThreadInputUpdateBody {
            content,
            after_input_id: after,
            native_action: selection,
            delivery,
        } = update;
        if matches!(
            (delivery, after),
            (Some(Delivery::Queue), Some(_))
                | (Some(Delivery::Steer | Delivery::Interrupt), Some(Some(_)))
        ) {
            return Err(InputError::Invalid(
                "the explicit position conflicts with the delivery's queue position",
            ));
        }
        if let Some(content) = &content {
            if content.is_empty() {
                return Err(InputError::Empty);
            }
            if text_chars(content) > usize::try_from(INPUT_TEXT_MAX_CHARS).unwrap_or(usize::MAX) {
                return Err(InputError::TooLong);
            }
        }
        let (finished, view, interrupt) = {
            let mut unit = Unit::new(&mut tx, now);
            let thread = threads::lock(&mut *unit.conn, thread_id)
                .await?
                .ok_or(InputError::NotFound)?;
            self.check_sender(&mut unit, caller, &thread, false).await?;
            let mut input = threads::input(&mut *unit.conn, input_id)
                .await?
                .filter(|input| input.thread_id == thread_id)
                .ok_or(InputError::NotFound)?;
            let recalling = input.state == input_state::DELIVERING
                && input.delivery == "steer"
                && delivery == Some(Delivery::Queue)
                && content.is_none()
                && selection.is_none()
                && after.is_none();
            if input.state != input_state::PENDING && !recalling {
                return Err(InputError::NotPending {
                    state: input.state.clone(),
                });
            }
            if recalling {
                // Dispatch takes this fence under the same thread lock before it can reach a program.
                // Once crossed, a missing acknowledgement cannot prove that recalling is safe.
                if input.state_detail.as_deref() == Some("steering") {
                    return Err(InputError::NotPending {
                        state: input.state.clone(),
                    });
                }
                let seq = unit.stamp(thread_id).await?;
                threads::return_input_in_place(
                    unit.conn,
                    input_id,
                    &[input_state::DELIVERING],
                    seq,
                )
                .await?;
            }
            if (content.is_some() || selection.is_some()) && input.author_id != Some(caller.user())
            {
                return Err(InputError::NotTheAuthor);
            }
            let owns_execution = input.continuation_id == thread.active_continuation_id;
            if !owns_execution && (delivery.is_some() || after.is_some() || selection.is_some()) {
                return Err(InputError::InactiveContinuation {
                    continuation_id: input.continuation_id,
                    active_continuation_id: thread.active_continuation_id,
                });
            }
            let changed_words = match &content {
                Some(content) => {
                    serde_json::to_value(content).map_err(EngineError::from)? != input.content
                }
                None => false,
            };
            let changed_selection = match &selection {
                Some(selection) => {
                    serde_json::to_value(selection).map_err(EngineError::from)?
                        != input.native_action.clone().unwrap_or(Json::Null)
                }
                None => false,
            };
            // These proposed facts drive scope admission; the ordinary writer below still performs
            // its existing paste/name materialization, which preserves the content's blob identities.
            if let Some(content) = &content {
                input.content = serde_json::to_value(content).map_err(EngineError::from)?;
            }
            if let Some(selected) = &selection {
                input.native_action = selected
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(EngineError::from)?;
            }
            if let Some(delivery) = delivery {
                input.delivery = delivery_word(delivery).into();
            }
            let active = threads::active_turn(&mut *unit.conn, thread_id).await?;
            let steered_turn = active.as_ref().filter(|turn| {
                delivery == Some(Delivery::Steer)
                    && turn.state == threads::turn_state::RUNNING
                    && turn.plan == input.plan
                    && !input.is_held()
                    && thread.state != thread_state::BLOCKED
            });
            let mut without_model = false;
            if changed_words || changed_selection {
                let authority =
                    narrowed_input_authority(input.submission_authority.as_ref(), caller)?;
                let seq = unit.stamp(thread_id).await?;
                threads::set_input_submission_authority(
                    unit.conn,
                    input_id,
                    authority.as_ref(),
                    seq,
                )
                .await?;
                input.submission_authority = authority;
                input.run_as = None;
                input.run_as_authority = None;
                let serving = match input_author_caller(
                    &mut unit,
                    thread_id,
                    &input,
                    self.inner.clock.as_ref(),
                )
                .await?
                {
                    Some(author) => {
                        self.check_authors_for_turn(&mut unit, &author, &thread, steered_turn)
                            .await?
                    }
                    None => Serving::WithoutModel { held: true },
                };
                without_model = matches!(serving, Serving::WithoutModel { .. });
                if serving == (Serving::WithoutModel { held: true }) {
                    threads::hold_input(unit.conn, input_id, Some(threads::HELD_MODEL_USE), seq)
                        .await?;
                    input.state_detail = Some(threads::HELD_MODEL_USE.to_owned());
                }
            }
            let selected_delivery = delivery.unwrap_or(match input.delivery.as_str() {
                "steer" => Delivery::Steer,
                "interrupt" => Delivery::Interrupt,
                _ => Delivery::Queue,
            });
            if delivery == Some(Delivery::Steer)
                && !without_model
                && active.as_ref().is_some_and(|turn| {
                    turn.state == threads::turn_state::RUNNING && turn.plan == input.plan
                })
                && !self.agent_steers(&mut unit, &thread).await?
            {
                return Err(InputError::SteerUnsupported);
            }
            // A command may accept only some delivery modes. Keep that declaration when only
            // delivery changes too; never turn a native invocation into ordinary text.
            let selected = match &selection {
                Some(selected) => selected.clone(),
                None => input
                    .native_action
                    .clone()
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(EngineError::from)?,
            };
            let verified = match selected
                .as_ref()
                .filter(|_| selection.is_some() || delivery.is_some())
            {
                Some(selected) => {
                    if selection.is_some() {
                        crate::native_actions::validate_selected(
                            &mut unit,
                            caller,
                            thread_id,
                            selected,
                            selected_delivery,
                        )
                        .await?;
                    }
                    let author = input_author_caller(
                        &mut unit,
                        thread_id,
                        &input,
                        self.inner.clock.as_ref(),
                    )
                    .await?
                    .ok_or(crate::native_actions::NativeError::Unavailable)?;
                    Some(
                        crate::native_actions::validate_for_input(
                            &mut unit,
                            &author,
                            thread_id,
                            selected,
                            selected_delivery,
                        )
                        .await?,
                    )
                }
                None => None,
            };
            if let Some(selection) = selection {
                let info=verified.as_ref().map(|verified| json!({"action_id":verified.id,"kind":verified.kind,"origin":verified.origin}));
                let raw = selection
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(EngineError::from)?;
                let seq = unit.stamp(thread_id).await?;
                sqlx::query("UPDATE thread_inputs SET native_action=$2,native_action_info=$3,seq_updated=$4,state_detail=CASE WHEN state_detail LIKE 'native_action.%' THEN NULL ELSE state_detail END WHERE id=$1")
                    .bind(input_id).bind(raw).bind(info).bind(seq).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
                threads::touch_queue(&mut *unit.conn, thread_id, seq).await?;
                unit.wake(thread_id);
            }
            let mut released_files = false;
            if let Some(content) = content {
                self.check_content(&mut unit, caller, &content).await?;
                let content = self.prepare_pastes(unit.conn, &thread, &content).await?;
                let seq = unit.stamp(thread_id).await?;
                let stored = serde_json::to_value(&content).map_err(EngineError::from)?;
                threads::update_input_content(&mut *unit.conn, input_id, &stored, seq).await?;
                orch_db::queries::storage::input_pins(
                    &mut *unit.conn,
                    input_id,
                    &orch_chat::following::content_blobs(&content),
                    now,
                )
                .await?;
                // Its author changed what its files held it for (removed the parts, say): it is looked at
                // again as it now stands, as a retry would.
                released_files =
                    threads::release_attachment_hold(&mut *unit.conn, input_id, seq).await?;
            }
            let mut interrupt = false;
            let mut steering = false;
            if let Some(delivery) = delivery {
                let seq = unit.stamp(thread_id).await?;
                threads::set_input_delivery(
                    &mut *unit.conn,
                    input_id,
                    delivery_word(delivery),
                    seq,
                )
                .await?;
                let candidate = threads::input(&mut *unit.conn, input_id)
                    .await?
                    .ok_or(InputError::NotFound)?;
                if delivery == Delivery::Steer
                    && !without_model
                    && let Some(turn) = active.as_ref().filter(|turn| {
                        turn.state == threads::turn_state::RUNNING && turn.plan == candidate.plan
                    })
                {
                    // Changing delivery records intent; it never lifts a Stop, a cap, a file
                    // hold or a blocked binding. Only a ready input joins the current turn.
                    if !candidate.is_held() && thread.state != thread_state::BLOCKED {
                        self.check_input_author_driving(&mut unit, &thread, &candidate)
                            .await?;
                        // Ordering another person's input never lends it the driver's provider
                        // rights. Its original author is checked again at the delivery boundary.
                        let sender =
                            input_caller(&mut unit, &thread, &candidate, self.inner.clock.as_ref())
                                .await?
                                .ok_or_else(|| {
                                    InputError::Denied(Box::new(Refusal::of(
                                        Permission::ProviderUse,
                                        ObjectRef {
                                            kind: ObjectKind::Provider,
                                            id: thread.provider_id.unwrap_or_else(Id::nil),
                                        },
                                        Vec::new(),
                                    )))
                                })?;
                        for caller in sender.callers() {
                            self.check_sender_for_turn(
                                &mut unit,
                                caller,
                                &thread,
                                true,
                                Some(turn),
                            )
                            .await?;
                        }
                        let needs_step = if let Some(author) = candidate.author_id {
                            crate::preflight::second_step_missing(
                                &mut *unit.conn,
                                author,
                                thread_id,
                            )
                            .await?
                        } else {
                            false
                        };
                        if needs_step {
                            threads::hold_input(
                                &mut *unit.conn,
                                input_id,
                                Some(threads::HELD_SECOND_STEP),
                                seq,
                            )
                            .await?;
                        } else {
                            threads::change_input(
                                &mut *unit.conn,
                                input_id,
                                &[input_state::PENDING],
                                &InputChange {
                                    state: input_state::DELIVERING,
                                    detail: candidate.state_detail.as_deref(),
                                    turn_id: Some(turn.id),
                                    ..InputChange::default()
                                },
                                seq,
                            )
                            .await?;
                            steering = true;
                        }
                    }
                }
                if delivery == Delivery::Interrupt
                    && !without_model
                    && let Some(turn) = &active
                {
                    threads::set_interrupt_cause(&mut *unit.conn, turn.id, "person").await?;
                    threads::mark_interrupted_by(&mut *unit.conn, turn.id, caller.user(), seq)
                        .await?;
                    interrupt = true;
                }
                unit.wake(thread_id);
            }
            if !recalling && !steering && (after.is_some() || delivery.is_some()) {
                let positioned = threads::queue_position_ids(&mut *unit.conn, thread_id).await?;
                let mut order: Vec<Id> = positioned
                    .iter()
                    .map(|(id, _)| *id)
                    .filter(|id| *id != input_id)
                    .collect();
                let place = match (delivery, after) {
                    (Some(Delivery::Queue), _) => order.len(),
                    (Some(Delivery::Steer | Delivery::Interrupt), _) | (None, Some(None)) => 0,
                    (None, Some(Some(other))) => {
                        if !positioned
                            .iter()
                            .any(|(id, pending)| *id == other && *pending)
                        {
                            return Err(InputError::Invalid("the input to follow is not waiting"));
                        }
                        order
                            .iter()
                            .position(|id| *id == other)
                            .ok_or(InputError::Invalid("the input to follow is not waiting"))?
                            + 1
                    }
                    (None, None) => order.len(),
                };
                order.insert(place, input_id);
                let seq = unit.stamp(thread_id).await?;
                threads::place_pending_inputs(&mut *unit.conn, thread_id, &order, seq).await?;
            }
            // Recheck only after every edit and its position are written. Pre-flight uses this
            // transaction's connection; files and harness commands run after commit.
            if owns_execution
                && (released_files || delivery.is_some())
                && active.is_none()
                && matches!(
                    thread.state.as_str(),
                    thread_state::IDLE | thread_state::BLOCKED
                )
            {
                self.preflight_in(&mut unit, thread_id).await?;
                unit.wake(thread_id);
            }
            let latest = threads::input(&mut *unit.conn, input_id)
                .await?
                .ok_or(InputError::NotFound)?;
            if changed_words || changed_selection {
                if latest.state == "pending"
                    && latest.state_detail.as_deref() == Some(threads::HELD_MODEL_USE)
                {
                    Box::pin(self.source_requested(
                        &mut unit,
                        &thread,
                        orch_db::queries::notifications::inbox::RequestSource::Input(input_id),
                        None,
                    ))
                    .await?;
                } else {
                    Box::pin(self.cancel_source_request(
                        &mut unit,
                        orch_db::queries::notifications::inbox::RequestSource::Input(input_id),
                        Some(caller.user()),
                    ))
                    .await?;
                }
            }
            let latest = threads::input(&mut *unit.conn, input_id)
                .await?
                .ok_or(InputError::NotFound)?;
            // Reordering writes the neighboring positions too. Return the same stamped
            // patch that the live feed reads, before releasing this thread's lock.
            let principal =
                Principal::load(&mut *unit.conn, caller.clone(), Time::new(now)).await?;
            let reads = nodes::check(
                &mut *unit.conn,
                &principal,
                thread_id,
                Permission::ThreadView,
                Act::Use,
                orch_authz::Policy::default(),
            )
            .await?
            .is_allowed();
            let changed = if reads {
                threads::inputs_since(&mut *unit.conn, thread_id, thread.seq()).await?
            } else {
                Vec::new()
            };
            let people = People::load(
                &mut *unit.conn,
                changed
                    .iter()
                    .chain(std::iter::once(&latest))
                    .flat_map(|row| {
                        row.author_id
                            .into_iter()
                            .chain(row.run_as)
                            .chain(views::bypass_people(row.limit_bypass.as_ref()))
                    }),
            )
            .await?;
            let people = people
                .with_approvals(
                    &mut *unit.conn,
                    changed
                        .iter()
                        .chain(std::iter::once(&latest))
                        .filter_map(|input| input.approval_id),
                )
                .await?;
            let view = ThreadInputUpdateReply {
                input: views::input_view(&latest, &people)?,
                inputs: changed
                    .iter()
                    .map(|row| views::input_view(row, &people))
                    .collect::<Result<Vec<_>, _>>()?,
            };
            (unit.finish().await?, view, interrupt)
        };
        self.commit(tx, finished).await?;
        if interrupt {
            self.drive_interrupt(thread_id).await?;
        }
        Ok(view)
    }

    /// Cancels an input that waits, whoever sent it.
    pub async fn cancel_input(
        &self,
        caller: &Caller,
        thread_id: Id,
        input_id: Id,
    ) -> Result<ThreadInput, InputError> {
        let tx = self.inner.db.begin().await?;
        let now = self.inner.clock.now();
        self.cancel_input_with(tx, caller, now, thread_id, input_id)
            .await
    }

    /// As [`Engine::cancel_input`], in the transaction of the API's action guard, with its caller and its time.
    pub async fn cancel_input_guarded(
        &self,
        tx: orch_db::Tx,
        principal: Principal,
        thread_id: Id,
        input_id: Id,
    ) -> Result<ThreadInput, InputError> {
        let caller = principal.caller().clone();
        let now = principal.now().get();
        self.cancel_input_with(tx, &caller, now, thread_id, input_id)
            .await
    }

    async fn cancel_input_with(
        &self,
        mut tx: orch_db::Tx,
        caller: &Caller,
        now: chrono::DateTime<chrono::Utc>,
        thread_id: Id,
        input_id: Id,
    ) -> Result<ThreadInput, InputError> {
        let (finished, view) = {
            let mut unit = Unit::new(&mut tx, now);
            let thread = threads::lock(&mut *unit.conn, thread_id)
                .await?
                .ok_or(InputError::NotFound)?;
            self.check_sender(&mut unit, caller, &thread, false).await?;
            let input = threads::input(&mut *unit.conn, input_id)
                .await?
                .filter(|input| input.thread_id == thread_id)
                .ok_or(InputError::NotFound)?;
            let seq = unit.stamp(thread_id).await?;
            let moved = threads::change_input(
                &mut *unit.conn,
                input_id,
                &[input_state::PENDING],
                &InputChange {
                    state: input_state::CANCELLED,
                    cancelled_by: Some(caller.user()),
                    ..InputChange::default()
                },
                seq,
            )
            .await?;
            if moved {
                Box::pin(self.cancel_source_request(
                    &mut unit,
                    orch_db::queries::notifications::inbox::RequestSource::Input(input_id),
                    Some(caller.user()),
                ))
                .await?;
            }
            if !moved && input.state != input_state::CANCELLED {
                return Err(InputError::NotPending {
                    state: input.state.clone(),
                });
            }
            // Words whose place was kept for a steer that did not reach the turn are taken back with the input.
            if moved {
                crate::ingest::settle_user_message(&mut unit, thread_id, input_id, "cancelled")
                    .await?;
                // Taking a goal's continuation out of the queue is saying stop to the goal: the loop would queue it again.
                self.goal_input_cancelled(&mut unit, &thread, &input, caller.user())
                    .await?;
            }
            // Taking out the last input a Stop held ends the hold.
            if moved && input.queue_held {
                if input.continuation_id == thread.active_continuation_id {
                    self.clear_emptied_hold(&mut unit, &thread, caller.user())
                        .await?;
                } else {
                    threads::clear_empty_inactive_hold(unit.conn, input_id).await?;
                }
            }
            if moved
                && input
                    .author_id
                    .is_some_and(|author| author != caller.user())
            {
                crate::audit_facts::person(
                    &mut *unit.conn,
                    caller.user(),
                    thread_id,
                    orch_db::audit::name!("input.cancelled"),
                    serde_json::json!({"input":input_id,"author":input.author_id}),
                    now,
                )
                .await?;
            }
            let latest = threads::input(&mut *unit.conn, input_id)
                .await?
                .ok_or(InputError::NotFound)?;
            let people = People::load(
                &mut *unit.conn,
                latest.author_id.into_iter().chain(latest.run_as),
            )
            .await?
            .with_approvals(&mut *unit.conn, latest.approval_id)
            .await?;
            let view = views::input_view(&latest, &people)?;
            (unit.finish().await?, view)
        };
        self.commit(tx, finished).await?;
        Ok(view)
    }

    /// Pins an item of a thread to its outline in the name of `user`, or takes
    /// the pin off. Whoever may drive the thread pins; that is asked before.
    pub async fn set_pin(
        &self,
        user: Id,
        thread_id: Id,
        item_id: Id,
        pinned: bool,
    ) -> Result<orch_api_contract::threads::Item, crate::views::ReadError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let (finished, item) = {
            let mut unit = Unit::new(&mut tx, now);
            threads::lock(&mut *unit.conn, thread_id)
                .await?
                .ok_or(crate::views::ReadError::NotFound)?;
            threads::item(&mut *unit.conn, item_id)
                .await?
                .filter(|row| row.thread_id == thread_id)
                .ok_or(crate::views::ReadError::NotFound)?;
            let seq = unit.stamp(thread_id).await?;
            threads::pin_item(&mut *unit.conn, item_id, pinned.then_some((user, now)), seq).await?;
            let row = threads::item(&mut *unit.conn, item_id)
                .await?
                .ok_or(crate::views::ReadError::NotFound)?;
            let people = People::load(
                &mut *unit.conn,
                [row.author_id, row.pinned_by]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>(),
            )
            .await?;
            (unit.finish().await?, views::item_view(&row, &people))
        };
        self.commit(tx, finished).await?;
        Ok(item)
    }

    /// Accepts a proposed plan once and queues one ordinary attributed input in the same transaction.
    pub async fn decide_plan_guarded(
        &self,
        tx: orch_db::Tx,
        principal: Principal,
        thread_id: Id,
        item_id: Id,
        body: &orch_api_contract::threads::ThreadPlanDecideBody,
    ) -> Result<orch_api_contract::threads::ThreadPlanDecideReply, InputError> {
        let (input, decision) = self
            .decide_plan_projected_guarded(
                tx,
                principal,
                thread_id,
                item_id,
                body,
                false,
                || Ok(()),
            )
            .await?;
        Ok(orch_api_contract::threads::ThreadPlanDecideReply {
            input: input.full()?.input,
            decision,
        })
    }

    /// The same native plan writer, with optional response content from admitted API disclosure.
    #[allow(
        clippy::too_many_arguments,
        reason = "native operands, disclosure and one transport boundary"
    )]
    pub async fn decide_plan_projected_guarded(
        &self,
        mut tx: orch_db::Tx,
        principal: Principal,
        thread_id: Id,
        item_id: Id,
        body: &orch_api_contract::threads::ThreadPlanDecideBody,
        receipt_only: bool,
        before_effect: impl FnOnce() -> Result<(), EngineError> + Send,
    ) -> Result<
        (
            SubmittedProjection,
            orch_api_contract::approvals::RequestDecisionStatus,
        ),
        InputError,
    > {
        let caller = principal.caller().clone();
        let now = principal.now().get();
        let (finished, reply) = {
            let mut unit = Unit::with_first_effect(&mut tx, now, before_effect);
            let thread = self
                .lock_source_control(
                    unit.conn,
                    thread_id,
                    orch_db::queries::notifications::inbox::RequestSource::Plan(item_id),
                    true,
                )
                .await?
                .ok_or(InputError::NotFound)?;
            let permission = crate::approvals::deciding_permission(
                &mut *unit.conn,
                thread_id,
                &json!({"request_kind":"plan"}),
            )
            .await?;
            if let Decision::Refused(refusal) = nodes::check(
                &mut *unit.conn,
                &principal,
                thread_id,
                permission,
                Act::Use,
                orch_authz::Policy::default(),
            )
            .await?
            {
                return Err(InputError::Denied(Box::new(refusal)));
            }
            let item = threads::item(&mut *unit.conn, item_id)
                .await?
                .filter(|item| item.thread_id == thread_id && item.kind == "plan")
                .ok_or(InputError::NotFound)?;
            if item.continuation_id != thread.active_continuation_id {
                return Err(InputError::Invalid(
                    "select the plan's active history before answering it",
                ));
            }
            if let Some(linked) = item
                .payload
                .get("approval_id")
                .and_then(Json::as_str)
                .and_then(|id| id.parse::<Id>().ok())
                && let Some(row) =
                    orch_db::queries::approvals::approval(&mut *unit.conn, linked).await?
                && row.state != "pending"
                && let Some(status) = crate::approvals::source_status(&mut *unit.conn, &row).await?
            {
                return Err(InputError::AlreadyDecided(Box::new(status)));
            }
            let turn = threads::turn(&mut *unit.conn, item.turn_id.ok_or(InputError::NotFound)?)
                .await?
                .filter(|turn| turn.plan && turn.state == "completed")
                .ok_or(InputError::Invalid("the plan has not finished"))?;
            let latest = threads::latest_active_turn(&mut *unit.conn, thread_id)
                .await?
                .ok_or(InputError::NotFound)?;
            if latest.id != turn.id
                || thread.current_turn_id.is_some()
                || threads::count_pending_inputs(&mut *unit.conn, thread_id).await? > 0
            {
                return Err(InputError::Invalid(
                    "the conversation has moved on from this plan",
                ));
            }
            let words = if body.proceed {
                "Implement the plan."
            } else {
                body.text
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or(InputError::Empty)?
            };
            let request = InputRequest {
                content: vec![ContentPart::Text {
                    text: words.to_owned(),
                }],
                delivery: Delivery::Queue,
                client_id: format!("plan:{item_id}"),
                bypass: None,
                plan: !body.proceed,
            };
            validate_literal_input(&request)?;
            let choice = json!({"option_id":if body.proceed {"proceed"}else{"keep_planning"},"note":body.text});
            let decision = Box::pin(self.claim_source(
                &mut unit,
                &thread,
                orch_db::queries::notifications::inbox::RequestSource::Plan(item_id),
                caller.user(),
                &choice,
            ))
            .await?;
            let seq = unit.stamp(thread_id).await?;
            sqlx::query("UPDATE items SET seq_updated=$2,updated_at=$3 WHERE id=$1")
                .bind(item_id)
                .bind(seq)
                .bind(now)
                .execute(&mut *unit.conn)
                .await
                .map_err(orch_db::DbError::from)?;
            let taken = Box::pin(self.submit_selected_made_row_in(
                &mut unit,
                InputExecution::Direct(&caller),
                thread_id,
                &request,
                None,
                Made::USER,
                None,
                None,
            ))
            .await?;
            let (mut submitted, _) = taken.project(unit.conn, receipt_only).await?;
            crate::audit_facts::person(
                &mut *unit.conn,
                caller.user(),
                thread_id,
                "approval.decided",
                json!({"plan_item_id":item_id,"decision":decision}),
                now,
            )
            .await?;
            submitted.seq = unit.seq_given(thread_id).unwrap_or(submitted.seq);
            (unit.finish().await?, (submitted, decision))
        };
        self.commit(tx, finished).await?;
        Ok(reply)
    }

    /// Runs an input that waits for somebody who may use the thread's model on the caller's own access: they must
    /// drive the thread and use its model and effort themselves. Its turn is theirs to answer for (usage, caps,
    /// the provider's entry); the author stays its author. Pre-flight runs again at once.
    pub async fn run_input(
        &self,
        caller: &Caller,
        thread_id: Id,
        input_id: Id,
    ) -> Result<ThreadInput, InputError> {
        let tx = self.inner.db.begin().await?;
        let now = self.inner.clock.now();
        Ok(self
            .run_input_with(tx, caller, now, thread_id, input_id, false, || Ok(()))
            .await?
            .full()?
            .input)
    }

    /// As [`Engine::run_input`], in the transaction of the API's action guard, with its caller and its time.
    pub async fn run_input_guarded(
        &self,
        tx: orch_db::Tx,
        principal: Principal,
        thread_id: Id,
        input_id: Id,
    ) -> Result<ThreadInput, InputError> {
        let caller = principal.caller().clone();
        let now = principal.now().get();
        Ok(self
            .run_input_with(tx, &caller, now, thread_id, input_id, false, || Ok(()))
            .await?
            .full()?
            .input)
    }

    /// The same native held-input writer with separately admitted optional response content.
    #[allow(
        clippy::too_many_arguments,
        reason = "native operands, disclosure and one transport boundary"
    )]
    pub async fn run_input_projected_guarded(
        &self,
        tx: orch_db::Tx,
        principal: Principal,
        thread: Id,
        input: Id,
        receipt_only: bool,
        before_effect: impl FnOnce() -> Result<(), EngineError> + Send,
    ) -> Result<SubmittedProjection, InputError> {
        let now = principal.now().get();
        self.run_input_with(
            tx,
            principal.caller(),
            now,
            thread,
            input,
            receipt_only,
            before_effect,
        )
        .await
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "native operands, disclosure and one transport boundary"
    )]
    async fn run_input_with(
        &self,
        mut tx: orch_db::Tx,
        caller: &Caller,
        now: chrono::DateTime<chrono::Utc>,
        thread_id: Id,
        input_id: Id,
        receipt_only: bool,
        before_effect: impl FnOnce() -> Result<(), EngineError> + Send,
    ) -> Result<SubmittedProjection, InputError> {
        let (finished, view) = {
            let mut unit = Unit::with_first_effect(&mut tx, now, before_effect);
            let thread = self
                .lock_source_control(
                    unit.conn,
                    thread_id,
                    orch_db::queries::notifications::inbox::RequestSource::Input(input_id),
                    true,
                )
                .await?
                .ok_or(InputError::NotFound)?;
            if thread.archived_at.is_some() {
                return Err(InputError::Archived);
            }
            let input = threads::input(&mut *unit.conn, input_id)
                .await?
                .filter(|input| input.thread_id == thread_id)
                .ok_or(InputError::NotFound)?;
            let raw_authority = input_authority(caller)?;
            let authority = saved_authority(Some(&raw_authority))?
                .ok_or_else(|| EngineError::unreadable("the run approval has no authority"))?;
            let current = input_authority_callers(
                &mut unit,
                &thread,
                &input,
                caller.user(),
                &authority,
                self.inner.clock.as_ref(),
            )
            .await?
            .ok_or_else(|| {
                InputError::Denied(Box::new(Refusal::of(
                    Permission::ThreadInteract,
                    ObjectRef {
                        kind: ObjectKind::Thread,
                        id: thread_id,
                    },
                    Vec::new(),
                )))
            })?;
            for caller in current.callers() {
                self.check_sender(&mut unit, caller, &thread, true).await?;
            }
            if input.continuation_id != thread.active_continuation_id {
                return Err(InputError::Invalid(
                    "select this message's active history before running it",
                ));
            }
            if let Some(linked) = input.approval_id
                && let Some(row) =
                    orch_db::queries::approvals::approval(&mut *unit.conn, linked).await?
                && row.state != "pending"
                && let Some(status) = crate::approvals::source_status(&mut *unit.conn, &row).await?
            {
                return Err(InputError::AlreadyDecided(Box::new(status)));
            }
            if input.state != input_state::PENDING {
                return Err(InputError::NotPending {
                    state: input.state.clone(),
                });
            }
            if input.state_detail.as_deref() != Some(threads::HELD_MODEL_USE) {
                return Err(InputError::Invalid(
                    "only a message that waits for somebody who may use the model can be run on your access",
                ));
            }
            let seq = unit.stamp(thread_id).await?;
            self.check_input_author_driving(&mut unit, &thread, &input)
                .await?;
            let mut provenance = derived_input_provenance(&input, None)?;
            provenance.run_as = Some(current.user());
            provenance.run_as_authority =
                Some(serde_json::to_value(current.captured()).map_err(EngineError::from)?);
            Box::pin(self.claim_source(
                &mut unit,
                &thread,
                orch_db::queries::notifications::inbox::RequestSource::Input(input_id),
                caller.user(),
                &json!({"run":true}),
            ))
            .await?;
            threads::set_input_run_as(
                &mut *unit.conn,
                input_id,
                provenance.run_as,
                provenance.run_as_authority.as_ref(),
                seq,
            )
            .await?;
            let action = audit::action!(AuditAction::ThreadInputRun);
            let mut entry = audit::Entry::new(
                Actor::User(caller.user()),
                action.as_str(),
                action.object_kind().unwrap_or("thread"),
                Some(thread_id),
                "A waiting message was run on the access of somebody who may use the thread's model",
            );
            entry.history = action.is_history_line();
            entry.node_id = Some(thread_id);
            entry.org_id = orch_db::usage::caps::node_org(&mut *unit.conn, thread_id).await?;
            entry.details = json!({ "input_id": input_id, "author_id": input.author_id });
            audit::append(&mut *unit.conn, &entry, &Origin::default(), now).await?;
            let active = threads::active_turn(&mut *unit.conn, thread_id).await?;
            let owns_execution = input.continuation_id == thread.active_continuation_id;
            if owns_execution
                && active.is_none()
                && matches!(
                    thread.state.as_str(),
                    thread_state::IDLE | thread_state::BLOCKED
                )
                && let Err(error) = self.preflight_in(&mut unit, thread_id).await
            {
                if matches!(error, EngineError::Db(_)) {
                    return Err(error.into());
                }
                tracing::warn!(%error, %thread_id, "pre-flight could not decide; the input waits");
            }
            if owns_execution {
                unit.wake(thread_id);
            }
            let latest = threads::input(&mut *unit.conn, input_id)
                .await?
                .ok_or(InputError::NotFound)?;
            let seq = unit.seq_given(thread_id).unwrap_or(thread.seq());
            let (projected, _) = TakenRow {
                row: latest,
                seq,
                created: false,
                interrupt: None,
            }
            .project(unit.conn, receipt_only)
            .await?;
            (unit.finish().await?, projected)
        };
        self.commit(tx, finished).await?;
        Ok(view)
    }

    /// Lets an input that waits go past the limits `asked` names, in the name
    /// of the caller, who must hold the bypass row of each. The caller's earlier
    /// entry is replaced; other people's stay. Pre-flight runs again at once,
    /// where a turn might start.
    pub async fn bypass_input(
        &self,
        caller: &Caller,
        thread_id: Id,
        input_id: Id,
        asked: &LimitBypassRequest,
    ) -> Result<ThreadInput, InputError> {
        let tx = self.inner.db.begin().await?;
        let now = self.inner.clock.now();
        Ok(self
            .bypass_input_with(
                tx,
                caller,
                now,
                thread_id,
                input_id,
                asked,
                false,
                None,
                || Ok(()),
            )
            .await?
            .full()?
            .input)
    }

    /// As [`Engine::bypass_input`], in the transaction of the API's action guard, with its caller and its time.
    pub async fn bypass_input_guarded(
        &self,
        tx: orch_db::Tx,
        principal: Principal,
        thread_id: Id,
        input_id: Id,
        asked: &LimitBypassRequest,
    ) -> Result<ThreadInput, InputError> {
        let caller = principal.caller().clone();
        let now = principal.now().get();
        Ok(self
            .bypass_input_with(
                tx,
                &caller,
                now,
                thread_id,
                input_id,
                asked,
                false,
                None,
                || Ok(()),
            )
            .await?
            .full()?
            .input)
    }

    /// The same native bypass writer with trusted current homes and optional full response.
    #[allow(
        clippy::too_many_arguments,
        reason = "same native request, trusted cap homes and optional disclosure stay separate"
    )]
    pub async fn bypass_input_projected_guarded(
        &self,
        tx: orch_db::Tx,
        principal: Principal,
        thread: Id,
        input: Id,
        asked: &LimitBypassRequest,
        receipt_only: bool,
        selected: &[orch_authz::caps::Home],
        before_effect: impl FnOnce() -> Result<(), EngineError> + Send,
    ) -> Result<SubmittedProjection, InputError> {
        let now = principal.now().get();
        self.bypass_input_with(
            tx,
            principal.caller(),
            now,
            thread,
            input,
            asked,
            receipt_only,
            Some(selected),
            before_effect,
        )
        .await
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "same native request, trusted actual cap homes and response disclosure are separate"
    )]
    async fn bypass_input_with(
        &self,
        mut tx: orch_db::Tx,
        caller: &Caller,
        now: chrono::DateTime<chrono::Utc>,
        thread_id: Id,
        input_id: Id,
        asked: &LimitBypassRequest,
        receipt_only: bool,
        selected: Option<&[orch_authz::caps::Home]>,
        before_effect: impl FnOnce() -> Result<(), EngineError> + Send,
    ) -> Result<SubmittedProjection, InputError> {
        let (finished, view) = {
            let mut unit = Unit::with_first_effect(&mut tx, now, before_effect);
            let thread = self
                .lock_source_control(
                    unit.conn,
                    thread_id,
                    orch_db::queries::notifications::inbox::RequestSource::Input(input_id),
                    false,
                )
                .await?
                .ok_or(InputError::NotFound)?;
            if thread.archived_at.is_some() {
                return Err(InputError::Archived);
            }
            self.check_sender(&mut unit, caller, &thread, false).await?;
            let input = threads::input(&mut *unit.conn, input_id)
                .await?
                .filter(|input| input.thread_id == thread_id)
                .ok_or(InputError::NotFound)?;
            if input.state != input_state::PENDING {
                return Err(InputError::NotPending {
                    state: input.state.clone(),
                });
            }
            let entry = self
                .bypass_of_from(
                    &mut unit,
                    caller,
                    &thread,
                    input.author_id,
                    Some(asked),
                    selected,
                )
                .await?;
            let mut kept: Vec<Json> = input
                .limit_bypass
                .as_ref()
                .and_then(Json::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|each| {
                    each.get("by").and_then(Json::as_str)
                        != Some(caller.user().to_string().as_str())
                })
                .collect();
            if let Some(Json::Array(mine)) = entry {
                kept.extend(mine);
            }
            let seq = unit.stamp(thread_id).await?;
            threads::set_input_bypass(&mut *unit.conn, input_id, &Json::Array(kept), seq).await?;
            let active = threads::active_turn(&mut *unit.conn, thread_id).await?;
            let owns_execution = input.continuation_id == thread.active_continuation_id;
            if owns_execution
                && active.is_none()
                && matches!(
                    thread.state.as_str(),
                    thread_state::IDLE | thread_state::BLOCKED
                )
                && let Err(error) = self.preflight_in(&mut unit, thread_id).await
            {
                if matches!(error, EngineError::Db(_)) {
                    return Err(error.into());
                }
                tracing::warn!(%error, %thread_id, "pre-flight could not decide; the input waits");
            }
            if owns_execution {
                unit.wake(thread_id);
            }
            let latest = threads::input(&mut *unit.conn, input_id)
                .await?
                .ok_or(InputError::NotFound)?;
            let people = latest
                .limit_bypass
                .as_ref()
                .map(|_| ())
                .map_or_else(Vec::new, |()| bypass_authors(&latest))
                .into_iter()
                .chain(latest.author_id)
                .chain(latest.run_as)
                .collect();
            let seq = unit.seq_given(thread_id).unwrap_or(thread.seq());
            let (projected, _) = TakenRow {
                row: latest,
                seq,
                created: false,
                interrupt: None,
            }
            .project_with_people(unit.conn, receipt_only, Some(people))
            .await?;
            (unit.finish().await?, projected)
        };
        self.commit(tx, finished).await?;
        Ok(view)
    }

    /// Records how far a person has read a thread; it only ever increases.
    pub async fn mark_read(&self, user: Id, thread_id: Id, seq: Seq) -> Result<Seq, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let Some(thread) = threads::thread(&mut conn, thread_id).await? else {
            return Err(EngineError::unreadable("there is no such thread"));
        };
        let seen = seq.clamp(0, thread.seq());
        Ok(threads::mark_read(&mut conn, user, thread_id, seen, self.inner.clock.now()).await?)
    }

    /// A person's unsent content for a thread.
    pub async fn draft(
        &self,
        user: Id,
        thread_id: Id,
    ) -> Result<Option<threads::DraftRow>, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        Ok(threads::draft(&mut conn, user, thread_id).await?)
    }

    /// Saves a person's unsent content for a thread.
    pub async fn set_draft(
        &self,
        user: Id,
        thread_id: Id,
        content: &[ContentPart],
        delivery: Delivery,
    ) -> Result<threads::DraftRow, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let stored = serde_json::to_value(content)?;
        Ok(threads::set_draft(
            &mut conn,
            user,
            thread_id,
            &stored,
            delivery_word(delivery),
            self.inner.clock.now(),
        )
        .await?)
    }

    /// Throws a person's draft away.
    pub async fn delete_draft(&self, user: Id, thread_id: Id) -> Result<bool, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        Ok(threads::delete_draft(&mut conn, user, thread_id).await?)
    }
}

// ---------------------------------------------------------------- what other agents and people announce

/// How an announcement, or a message from another agent, reaches the thread that is to hear it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Arrival {
    /// First in the queue, with the turn that runs stopped, in the name of the person who answers for the sender.
    Interrupt {
        /// Who answers for the interrupt.
        responsible: Id,
    },
    /// Into the turn that runs.
    Steer {
        /// That turn.
        turn: Id,
    },
    /// First in the queue, for the turn that comes next.
    Front,
}

impl Engine {
    /// Makes an input of what was announced to a thread, in the transaction of `unit`. It has no author: at
    /// pre-flight the thread's first owner is asked in its place. `id` makes it once; the thread's row must be
    /// locked by the caller.
    pub(crate) async fn queue_announcement_input(
        &self,
        unit: &mut Unit<'_>,
        thread_id: Id,
        id: Id,
        text: &str,
        files: &[ContentPart],
        arrival: Arrival,
    ) -> Result<(), EngineError> {
        let now = unit.now();
        // What a message to one thread carries goes with it as file parts, which the courier writes before the
        // turn begins or the steer is sent.
        let mut parts = vec![ContentPart::Text {
            text: text.to_owned(),
        }];
        parts.extend_from_slice(files);
        let content = serde_json::to_value(parts)?;
        let (delivery, state, turn, place) = match arrival {
            Arrival::Interrupt { .. } => ("interrupt", input_state::PENDING, None, Place::Front),
            Arrival::Steer { turn } => ("steer", input_state::DELIVERING, Some(turn), Place::End),
            Arrival::Front => ("queue", input_state::PENDING, None, Place::Front),
        };
        let seq = unit.stamp(thread_id).await?;
        let made = threads::insert_input(
            &mut *unit.conn,
            &NewInput {
                plan: false,
                id,
                thread_id,
                author_id: None,
                submission_authority: None,
                run_as: None,
                run_as_authority: None,
                source: "announcement",
                content: &content,
                delivery,
                state,
                turn_id: turn,
                client_id: None,
                limit_bypass: None,
                place,
                at: now,
            },
            seq,
        )
        .await?;
        let Some(made) = made else {
            return Ok(());
        };
        let active = threads::active_turn(&mut *unit.conn, thread_id).await?;
        if !made.queue_held
            && let (Arrival::Interrupt { responsible }, Some(turn)) = (arrival, &active)
        {
            let seq = unit.stamp(thread_id).await?;
            // An input no person sent cuts the turn by the topic's setting of its delivery, not by a person.
            threads::set_interrupt_cause(&mut *unit.conn, turn.id, "topic_setting").await?;
            threads::mark_interrupted_by(&mut *unit.conn, turn.id, responsible, seq).await?;
        }
        if !made.queue_held
            && active.is_none()
            && let Err(error) = self.preflight_in(unit, thread_id).await
        {
            if matches!(error, EngineError::Db(_)) {
                return Err(error);
            }
            tracing::warn!(%error, %thread_id, "pre-flight could not decide; the input waits");
        }
        unit.wake(thread_id);
        Ok(())
    }
}

// ---------------------------------------------------------------- what a person's Stop holds

impl Engine {
    /// Holds, for a person's Stop, the inputs that wait in a thread's queue now, under the thread's lock
    /// which the caller holds (it is the lock that accepting an input and starting a turn take, so an input
    /// accepted before the Stop is held, and later arrivals inherit that hold until Resume). The thread records who stopped
    /// and when, its history says so in one line, and the audit in one row. Where nothing waits nothing is
    /// held and no line is written: a Stop leaves no empty hold.
    pub(crate) async fn hold_queue(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        by: Id,
    ) -> Result<(), EngineError> {
        let seq = unit.stamp(thread.node_id).await?;
        let now = unit.now();
        let (held, revision) =
            threads::hold_queue(&mut *unit.conn, thread.node_id, by, now, seq).await?;
        if held == 0 {
            return Ok(());
        }
        self.queue_line(unit, thread, "queue_held", by, held, revision)
            .await?;
        crate::audit_facts::person(
            &mut *unit.conn,
            by,
            thread.node_id,
            orch_db::audit::name!("queue.held"),
            json!({ "inputs": held, "revision": revision }),
            now,
        )
        .await?;
        Ok(())
    }

    /// Resumes what a person's Stop held, for whoever may drive the thread, naming the revision of the
    /// queue they saw: the held inputs keep their places, authors and every other hold of theirs, and
    /// pre-flight asks of each author as usual. A revision that is no longer the one that stands is
    /// refused with the one that does; a queue that holds nothing is answered as it is.
    pub async fn resume_queue(
        &self,
        caller: &Caller,
        thread_id: Id,
        revision: i64,
    ) -> Result<orch_api_contract::threads::ThreadQueue, InputError> {
        let tx = self.inner.db.begin().await?;
        let now = self.inner.clock.now();
        self.resume_queue_with(tx, caller, now, thread_id, revision)
            .await
    }

    /// As [`Engine::resume_queue`], in the transaction of the API's action guard, with its caller and its time.
    pub async fn resume_queue_guarded(
        &self,
        tx: orch_db::Tx,
        principal: Principal,
        thread_id: Id,
        revision: i64,
    ) -> Result<orch_api_contract::threads::ThreadQueue, InputError> {
        let caller = principal.caller().clone();
        let now = principal.now().get();
        self.resume_queue_with(tx, &caller, now, thread_id, revision)
            .await
    }

    async fn resume_queue_with(
        &self,
        mut tx: orch_db::Tx,
        caller: &Caller,
        now: chrono::DateTime<chrono::Utc>,
        thread_id: Id,
        revision: i64,
    ) -> Result<orch_api_contract::threads::ThreadQueue, InputError> {
        let (finished, queue) = {
            let mut unit = Unit::new(&mut tx, now);
            let thread = threads::lock(&mut *unit.conn, thread_id)
                .await?
                .ok_or(InputError::NotFound)?;
            if thread.archived_at.is_some() {
                return Err(InputError::Archived);
            }
            self.check_sender(&mut unit, caller, &thread, false).await?;
            if thread.queue_revision != revision {
                return Err(InputError::StaleQueue {
                    revision: thread.queue_revision,
                });
            }
            if thread.queue_held {
                let seq = unit.stamp(thread_id).await?;
                let (freed, after) = threads::resume_queue(&mut *unit.conn, thread_id, seq).await?;
                self.queue_line(
                    &mut unit,
                    &thread,
                    "queue_resumed",
                    caller.user(),
                    freed,
                    after,
                )
                .await?;
                crate::audit_facts::person(
                    &mut *unit.conn,
                    caller.user(),
                    thread_id,
                    orch_db::audit::name!("queue.resumed"),
                    json!({ "inputs": freed, "revision": after }),
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
            }
            let row = threads::thread(&mut *unit.conn, thread_id)
                .await?
                .ok_or(InputError::NotFound)?;
            let queue = views::queue_of(&mut *unit.conn, &row).await?;
            (unit.finish().await?, queue)
        };
        self.commit(tx, finished).await?;
        Ok(queue)
    }

    /// Ends a thread's queue hold once none of the inputs it held waits any more (the last was removed),
    /// in the caller's transaction, with the line that says it was let go.
    pub(crate) async fn clear_emptied_hold(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        by: Id,
    ) -> Result<(), EngineError> {
        if !thread.queue_held {
            return Ok(());
        }
        let seq = unit.stamp(thread.node_id).await?;
        if threads::clear_empty_queue_hold(&mut *unit.conn, thread.node_id, seq).await? {
            let row = threads::thread(&mut *unit.conn, thread.node_id).await?;
            let revision = row.map_or(thread.queue_revision, |row| row.queue_revision);
            self.queue_line(unit, thread, "queue_resumed", by, 0, revision)
                .await?;
        }
        Ok(())
    }

    /// The line of a thread's history that says its queue was held or let go. A thread that never ran has
    /// no history to write it in, and nothing to hold either.
    async fn queue_line(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        marker: &str,
        by: Id,
        inputs: i64,
        revision: i64,
    ) -> Result<(), EngineError> {
        let segment = match thread.current_segment_id {
            Some(segment) => Some(segment),
            None => threads::latest_turn(&mut *unit.conn, thread.node_id)
                .await?
                .map(|turn| turn.segment_id),
        };
        let Some(segment) = segment else {
            return Ok(());
        };
        let by = views::user_ref_json(&mut *unit.conn, by).await?;
        let payload = json!({
            "marker": marker,
            "data": { "by": by, "inputs": inputs, "revision": revision },
        });
        crate::ingest::add_marker(
            unit,
            thread.node_id,
            segment,
            None,
            &payload,
            &format!("{marker}:{revision}"),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_length_of_an_input_counts_what_is_said_and_not_what_is_attached() {
        let content = vec![
            ContentPart::Text {
                text: "héllo".into(),
            },
            ContentPart::File {
                path: "a".into(),
                name: "a".into(),
                size: 9_000,
                blob: None,
            },
        ];
        assert_eq!(text_chars(&content), 5);
    }
}
