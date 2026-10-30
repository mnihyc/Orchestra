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

use crate::access::{Limit, ProviderUse};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::preflight::Outcome as PreflightOutcome;
use crate::unit::Unit;
use crate::views::{self, People};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::{Decision, ObjectRef, Refusal, Scopes};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::threads::{
    ContentPart, Delivery, INPUT_TEXT_MAX_CHARS, LimitBypassRequest, ThreadInput,
    ThreadInputUpdateBody, ThreadInputUpdateReply,
};
use orch_authz::{Act, Caller, Principal, TokenGrant, nodes};
use orch_db::audit::{self, Actor, Origin};
use orch_db::queries::threads::{
    self, InputChange, InputRow, NewInput, Place, ThreadRow, TurnRow, input_state, thread_state,
};
use orch_types::{Id, Json, Seq, Time, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;

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

/// Immutable credential boundaries of accepted words; later edits may narrow but never widen them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InputAuthority {
    credentials: Vec<CredentialAuthority>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum CredentialAuthority {
    Session {
        #[serde(default)]
        as_admin: bool,
    },
    Token {
        token_id: Option<Id>,
        scopes: Vec<String>,
        node_id: Option<Id>,
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

fn credential_authority(caller: &Caller) -> CredentialAuthority {
    match caller.grant() {
        None => CredentialAuthority::Session {
            as_admin: caller.as_admin(),
        },
        Some(grant) => CredentialAuthority::Token {
            token_id: grant.token(),
            scopes: grant.scopes().to_strings(),
            node_id: grant.subtree(),
        },
    }
}

/// Records the accepted caller, rather than reconstructing a browser from their user id later.
pub(crate) fn input_authority(caller: &Caller) -> Result<Json, EngineError> {
    Ok(serde_json::to_value(InputAuthority {
        credentials: vec![credential_authority(caller)],
    })?)
}

/// Missing provenance is unknown, not an unrestricted browser submission.
pub(crate) fn saved_authority(raw: Option<&Json>) -> Result<Option<InputAuthority>, EngineError> {
    let authority: Option<InputAuthority> = raw.cloned().map(serde_json::from_value).transpose()?;
    if authority
        .as_ref()
        .is_some_and(|authority| authority.credentials.is_empty())
    {
        return Err(EngineError::unreadable(
            "input authority has no credential boundary",
        ));
    }
    Ok(authority)
}

/// Changing accepted words retains every earlier boundary as well as the current editor's.
pub(crate) fn narrowed_input_authority(
    raw: Option<&Json>,
    caller: &Caller,
) -> Result<Option<Json>, EngineError> {
    let Some(mut authority) = saved_authority(raw)? else {
        return Ok(None);
    };
    let current = credential_authority(caller);
    if !authority.credentials.contains(&current) {
        authority.credentials.push(current);
    }
    Ok(Some(serde_json::to_value(authority)?))
}

/// Intersects every accepted boundary with current token restrictions in the admission transaction.
pub(crate) async fn authority_caller(
    conn: &mut orch_db::Conn,
    user: Id,
    node: Id,
    now: Time,
    authority: &InputAuthority,
) -> Result<Option<Caller>, EngineError> {
    let mut scopes = Scopes::All;
    let mut root = None;
    let mut as_admin = true;
    let mut is_token = false;
    let mut kept_out = Vec::new();
    let mut token_ids = std::collections::BTreeSet::new();
    let mut synthetic_token = false;
    for credential in &authority.credentials {
        match credential {
            CredentialAuthority::Session { as_admin: selected } => as_admin &= *selected,
            CredentialAuthority::Token {
                token_id,
                scopes: accepted,
                node_id,
            } => {
                is_token = true;
                let recorded =
                    TokenGrant::from_stored(accepted, *node_id).map_err(EngineError::unreadable)?;
                let (current, current_root) = if let Some(token) = token_id {
                    token_ids.insert(*token);
                    let current: Option<(Vec<String>, Option<Id>)> = sqlx::query_as(
                        "SELECT scopes,node_id FROM api_tokens WHERE id=$1 AND user_id=$2 AND revoked_at IS NULL
                          AND (expires_at IS NULL OR expires_at>$3) FOR SHARE",
                    ).bind(token).bind(user).bind(now.get()).fetch_optional(&mut *conn)
                        .await.map_err(orch_db::DbError::from)?;
                    let Some((current, subtree)) = current else {
                        return Ok(None);
                    };
                    // The final caller combines several credentials, so carry each token's live policy restrictions.
                    for blocked in orch_db::queries::tokens::blocked(conn, *token).await? {
                        match (blocked.refused, blocked.org, blocked.max_days) {
                            (true, Some(org), _) => {
                                kept_out.push(orch_authz::KeptOut::Refused { org })
                            }
                            (false, place, Some(max_days)) => {
                                kept_out.push(orch_authz::KeptOut::Lifetime { place, max_days })
                            }
                            _ => {}
                        }
                    }
                    (
                        TokenGrant::from_stored(&current, subtree)
                            .map_err(EngineError::unreadable)?,
                        subtree,
                    )
                } else {
                    // Synthetic internal callers have no stored credential, but their recorded narrowing still binds.
                    synthetic_token = true;
                    (recorded.clone(), *node_id)
                };
                scopes = Scopes::Only(
                    Permission::ALL
                        .iter()
                        .copied()
                        .filter(|permission| {
                            scopes.allow(*permission)
                                && recorded.carries(*permission)
                                && current.carries(*permission)
                        })
                        .collect(),
                );
                for next in [*node_id, current_root].into_iter().flatten() {
                    root = match root {
                        None => Some(next),
                        Some(previous) if previous == next => Some(previous),
                        Some(previous) => {
                            let path: Option<Vec<Id>> =
                                sqlx::query_scalar("SELECT path FROM nodes WHERE id=$1")
                                    .bind(node)
                                    .fetch_optional(&mut *conn)
                                    .await
                                    .map_err(orch_db::DbError::from)?;
                            let Some(path) = path else { return Ok(None) };
                            match (
                                path.iter().position(|id| *id == previous),
                                path.iter().position(|id| *id == next),
                            ) {
                                (Some(a), Some(b)) => Some(if a > b { previous } else { next }),
                                _ => return Ok(None),
                            }
                        }
                    };
                }
            }
        }
    }
    Ok(Some(if is_token {
        let mut grant = TokenGrant::new(scopes, root).kept_out(kept_out);
        if !synthetic_token
            && token_ids.len() == 1
            && let Some(token) = token_ids.first()
        {
            grant = grant.for_token(*token);
        }
        Caller::token(user, grant)
    } else {
        Caller::session(user).with_admin_mode(as_admin)
    }))
}

/// The recorded execution actor; an explicit approver never replaces the author's original provenance.
pub(crate) async fn input_caller(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    input: &InputRow,
) -> Result<Option<Caller>, EngineError> {
    let (person, raw) = match input.run_as {
        Some(person) => (person, input.run_as_authority.as_ref()),
        None => match input.author_id {
            Some(person) => (person, input.submission_authority.as_ref()),
            None if input.is_authorless() => {
                return Ok(Some(
                    Caller::session(thread.owner_id).with_admin_mode(false),
                ));
            }
            None => return Ok(None),
        },
    };
    let Some(authority) = saved_authority(raw)? else {
        return Ok(None);
    };
    let now = Time::new(unit.now());
    authority_caller(unit.conn, person, thread.node_id, now, &authority).await
}

/// The author's original boundary, independent of any later model-use approver.
pub(crate) async fn input_author_caller(
    unit: &mut Unit<'_>,
    node: Id,
    input: &InputRow,
) -> Result<Option<Caller>, EngineError> {
    let Some(author) = input.author_id else {
        return Ok(None);
    };
    let Some(authority) = saved_authority(input.submission_authority.as_ref())? else {
        return Ok(None);
    };
    let now = Time::new(unit.now());
    authority_caller(unit.conn, author, node, now, &authority).await
}

impl Engine {
    pub(crate) async fn check_content(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        content: &[ContentPart],
    ) -> Result<(), InputError> {
        let hashes = orch_chat::following::content_blobs(content);
        if hashes.is_empty() {
            return Ok(());
        }
        let principal = Principal::load(unit.conn, caller.clone(), self.inner.clock.time()).await?;
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
        let original = match input_author_caller(unit, thread.node_id, input).await? {
            Some(original) => original,
            // Unknown historical provenance grants no model use. Its author's current
            // ordinary driving rights still bind an explicitly approved input.
            None if input.submission_authority.is_none() => {
                Caller::session(author).with_admin_mode(false)
            }
            None => return Err(denied()),
        };
        self.check_sender(unit, &original, thread, false).await
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
                .ok_or_else(|| {
                    EngineError::unreadable("the running turn's provider is unavailable")
                })?;
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
            let answer = self
                .inner
                .access
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

    async fn submit_input_with(
        &self,
        mut tx: orch_db::Tx,
        caller: &Caller,
        now: chrono::DateTime<chrono::Utc>,
        thread_id: Id,
        request: InputRequest,
        selection: Option<orch_api_contract::input_schedules::NativeInvocation>,
    ) -> Result<Submitted, InputError> {
        let (finished, taken) = {
            let mut unit = Unit::new(&mut tx, now);
            let mut taken = self
                .submit_selected_in(&mut unit, caller, thread_id, &request, selection.as_ref())
                .await?;
            let finished = unit.finish().await?;
            if taken.submitted.created {
                taken.submitted.seq = finished
                    .touched
                    .get(&thread_id)
                    .copied()
                    .unwrap_or(taken.submitted.seq);
            }
            (finished, taken)
        };
        self.commit(tx, finished).await?;
        if taken.interrupt.is_some() {
            self.drive_interrupt(thread_id).await?;
        }
        Ok(taken.submitted)
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

    /// Takes an input that a goal's loop makes in the name of the person who set the goal: every check a person's
    /// input passes, and the same rows, with the source and the goal it belongs to.
    pub(crate) async fn submit_made_in(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        thread_id: Id,
        request: &InputRequest,
        made: Made<'_>,
    ) -> Result<Taken, InputError> {
        self.submit_selected_made_in(unit, caller, thread_id, request, None, made)
            .await
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
        if selection.is_none() {
            validate_literal_input(request)?;
        }
        if text_chars(&request.content)
            > usize::try_from(INPUT_TEXT_MAX_CHARS).unwrap_or(usize::MAX)
        {
            return Err(InputError::TooLong);
        }
        if request.client_id.trim().is_empty() {
            return Err(InputError::Invalid(
                "an input carries the value that makes sending it again safe",
            ));
        }
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
            Some(caller.user()),
            &request.client_id,
        )
        .await?
        {
            let people = People::load(
                &mut *unit.conn,
                found.author_id.into_iter().chain(found.run_as),
            )
            .await?;
            let seq = thread.seq();
            let people = people
                .with_approvals(&mut *unit.conn, found.approval_id)
                .await?;
            let input = views::input_view(&found, &people)?;
            return Ok(Taken {
                submitted: Submitted {
                    input,
                    seq,
                    created: false,
                },
                interrupt: None,
            });
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
        // Boxed: a message is taken deep in a request's stack. A steer uses
        // the running turn's recorded provider, model and effort.
        let serving =
            Box::pin(self.check_author_for_turn(unit, caller, &thread, steered_turn)).await?;
        // A message that waits for somebody else's access joins the queue: it neither steers nor stops the turn
        // that runs on another person's access.
        let without_model = matches!(serving, Serving::WithoutModel { .. });
        if request.plan && !Box::pin(self.agent_plans(unit, &thread)).await? {
            return Err(InputError::PlanUnsupported);
        }
        self.check_content(unit, caller, &request.content).await?;
        let verified = match selection {
            Some(selection) => Some(
                crate::native_actions::validate_selected(
                    unit,
                    caller,
                    thread_id,
                    selection,
                    request.delivery,
                )
                .await?,
            ),
            None => None,
        };
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
        let bypass = self
            .bypass_of(
                unit,
                caller,
                &thread,
                Some(caller.user()),
                request.bypass.as_ref(),
            )
            .await?;
        let mut prepared = self
            .prepare_pastes(unit.conn, &thread, &request.content)
            .await?;
        self.prepare_attachment_names(unit.conn, thread_id, &mut prepared)
            .await?;
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
        let input_id = new_id();
        let authority = input_authority(caller)?;
        let inserted = threads::insert_input(
            &mut *unit.conn,
            &NewInput {
                plan: request.plan,
                id: input_id,
                thread_id,
                author_id: Some(caller.user()),
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
            crate::notifications::follow_thread(&mut *unit.conn, caller.user(), thread_id, now)
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
                Some(caller.user()),
            ))
            .await?;
        }
        if !row.queue_held
            && !without_model
            && request.delivery == Delivery::Interrupt
            && let Some(turn) = &active
        {
            threads::set_interrupt_cause(&mut *unit.conn, turn.id, "person").await?;
            threads::mark_interrupted_by(&mut *unit.conn, turn.id, caller.user(), seq).await?;
            interrupt_turn = Some(turn.id);
        }
        // A person's new input is an explicit decision to proceed after an attention stop; a goal's loop is not a person.
        if made.source == "user" {
            sqlx::query("UPDATE model_call_recovery SET state='completed',due_at=NULL,context=context-'manual_stop'-'manual_request' WHERE node_id=$1 AND purpose='agent' AND state IN ('waiting','stopped')")
                .bind(thread_id).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        }
        // A turn might start: the checks run in this transaction.
        if !row.queue_held
            && active.is_none()
            && matches!(
                thread.state.as_str(),
                thread_state::IDLE | thread_state::BLOCKED
            )
        {
            let outcome = match self.preflight_in(unit, thread_id).await {
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
        let people = People::load(
            &mut *unit.conn,
            latest.author_id.into_iter().chain(latest.run_as),
        )
        .await?
        .with_approvals(&mut *unit.conn, latest.approval_id)
        .await?;
        let input = views::input_view(&latest, &people)?;
        Ok(Taken {
            submitted: Submitted {
                input,
                seq,
                created: true,
            },
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
    async fn bypass_of(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        thread: &ThreadRow,
        author: Option<Id>,
        asked: Option<&LimitBypassRequest>,
    ) -> Result<Option<Json>, InputError> {
        let Some(asked) = asked else {
            return Ok(None);
        };
        let named: Vec<Id> = asked.caps.clone().unwrap_or_default();
        let provider_limit = asked.provider_limit == Some(true);
        if named.is_empty() && !provider_limit {
            return Ok(None);
        }
        let Some(author) = author else {
            return Err(InputError::Invalid(
                "an input that no person sent is never let past a limit",
            ));
        };
        let caps = self
            .caps_to_pass(unit, caller, thread, author, &named)
            .await?;
        if provider_limit {
            let Some(provider) = thread.provider_id else {
                return Err(InputError::Invalid(
                    "a thread with no provider has no limit to pass",
                ));
            };
            let holds = self
                .inner
                .access
                .may_pass(
                    &mut *unit.conn,
                    caller.user(),
                    caller.as_admin(),
                    &Limit::ProviderLimit { provider },
                )
                .await
                .map_err(EngineError::from)?;
            if !holds {
                return Err(InputError::Denied(Box::new(Refusal::of(
                    Permission::ProviderBypass,
                    ObjectRef {
                        kind: ObjectKind::Provider,
                        id: provider,
                    },
                    Vec::new(),
                ))));
            }
        }
        if caps.is_empty() && !provider_limit {
            return Ok(None);
        }
        let mut entry = json!({
            "by": caller.user(), "at": views::time_json(self.inner.clock.now()), "caps": caps,
            "provider_limit": provider_limit,
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
    async fn caps_to_pass(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        thread: &ThreadRow,
        author: Id,
        named: &[Id],
    ) -> Result<Vec<Id>, InputError> {
        if named.is_empty() {
            return Ok(Vec::new());
        }
        let settings = crate::resolved::load(&mut *unit.conn, thread.node_id).await?;
        let Some(ask) = self.ask_of_thread(unit, thread, &settings).await? else {
            return Ok(Vec::new());
        };
        let ask = crate::caps::Ask {
            person: Some(author),
            ..ask
        };
        let now = unit.now();
        let applied = crate::caps::covering(&mut *unit.conn, &ask, now).await?;
        let principal = Principal::load(&mut *unit.conn, caller.clone(), Time::new(now)).await?;
        let mut passes: Vec<Id> = Vec::new();
        for id in named {
            if passes.contains(id) {
                continue;
            }
            let Some(each) = applied.iter().find(|each| {
                each.cap.id == *id && each.replaced_by.is_none() && each.cap.amount.is_some()
            }) else {
                continue;
            };
            let standing =
                crate::caps::standing(&mut *unit.conn, &each.cap, Some(author), now).await?;
            if standing.has_room() {
                continue;
            }
            match orch_authz::caps::check_pass(
                &mut *unit.conn,
                &principal,
                crate::caps::home_of(&each.cap),
                orch_authz::Policy::default(),
            )
            .await?
            {
                orch_authz::caps::Verdict::Allowed => passes.push(*id),
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
                let serving = match input_author_caller(&mut unit, thread_id, &input).await? {
                    Some(author) => {
                        self.check_author_for_turn(&mut unit, &author, &thread, steered_turn)
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
                    let author = input_author_caller(&mut unit, thread_id, &input)
                        .await?
                        .ok_or(crate::native_actions::NativeError::Unavailable)?;
                    Some(
                        crate::native_actions::validate_selected(
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
                let candidate = threads::input(&mut *unit.conn, input_id)
                    .await?
                    .ok_or(InputError::NotFound)?;
                let seq = unit.stamp(thread_id).await?;
                threads::set_input_delivery(
                    &mut *unit.conn,
                    input_id,
                    delivery_word(delivery),
                    seq,
                )
                .await?;
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
                        let sender = input_caller(&mut unit, &thread, &candidate)
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
                        self.check_sender_for_turn(&mut unit, &sender, &thread, true, Some(turn))
                            .await?;
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
        mut tx: orch_db::Tx,
        principal: Principal,
        thread_id: Id,
        item_id: Id,
        body: &orch_api_contract::threads::ThreadPlanDecideBody,
    ) -> Result<orch_api_contract::threads::ThreadPlanDecideReply, InputError> {
        let caller = principal.caller().clone();
        let now = principal.now().get();
        let (finished, reply) = {
            let mut unit = Unit::new(&mut tx, now);
            let thread = threads::lock(&mut *unit.conn, thread_id)
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
            let taken =
                Box::pin(self.submit_input_in(&mut unit, &caller, thread_id, &request)).await?;
            crate::audit_facts::person(
                &mut *unit.conn,
                caller.user(),
                thread_id,
                "approval.decided",
                json!({"plan_item_id":item_id,"decision":decision}),
                now,
            )
            .await?;
            (
                unit.finish().await?,
                orch_api_contract::threads::ThreadPlanDecideReply {
                    input: taken.submitted.input,
                    decision,
                },
            )
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
        self.run_input_with(tx, caller, now, thread_id, input_id)
            .await
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
        self.run_input_with(tx, &caller, now, thread_id, input_id)
            .await
    }

    async fn run_input_with(
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
            if thread.archived_at.is_some() {
                return Err(InputError::Archived);
            }
            let authority = InputAuthority {
                credentials: vec![credential_authority(caller)],
            };
            let current = authority_caller(
                unit.conn,
                caller.user(),
                thread_id,
                Time::new(now),
                &authority,
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
            self.check_sender(&mut unit, &current, &thread, true)
                .await?;
            let input = threads::input(&mut *unit.conn, input_id)
                .await?
                .filter(|input| input.thread_id == thread_id)
                .ok_or(InputError::NotFound)?;
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
            let provenance = derived_input_provenance(&input, Some(&current))?;
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
            let people = People::load(
                &mut *unit.conn,
                latest
                    .author_id
                    .into_iter()
                    .chain(latest.run_as)
                    .collect::<Vec<_>>(),
            )
            .await?
            .with_approvals(&mut *unit.conn, latest.approval_id)
            .await?;
            (unit.finish().await?, views::input_view(&latest, &people)?)
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
        self.bypass_input_with(tx, caller, now, thread_id, input_id, asked)
            .await
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
        self.bypass_input_with(tx, &caller, now, thread_id, input_id, asked)
            .await
    }

    async fn bypass_input_with(
        &self,
        mut tx: orch_db::Tx,
        caller: &Caller,
        now: chrono::DateTime<chrono::Utc>,
        thread_id: Id,
        input_id: Id,
        asked: &LimitBypassRequest,
    ) -> Result<ThreadInput, InputError> {
        let (finished, view) = {
            let mut unit = Unit::new(&mut tx, now);
            let thread = threads::lock(&mut *unit.conn, thread_id)
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
                .bypass_of(&mut unit, caller, &thread, input.author_id, Some(asked))
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
            let people = People::load(
                &mut *unit.conn,
                latest
                    .limit_bypass
                    .as_ref()
                    .map(|_| ())
                    .map_or_else(Vec::new, |()| bypass_authors(&latest))
                    .into_iter()
                    .chain(latest.author_id)
                    .chain(latest.run_as)
                    .collect::<Vec<_>>(),
            )
            .await?
            .with_approvals(&mut *unit.conn, latest.approval_id)
            .await?;
            (unit.finish().await?, views::input_view(&latest, &people)?)
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
