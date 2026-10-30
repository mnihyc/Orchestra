//! Harness, access and capability decisions; access requests asked for and decided part by part, each sent to the nearest who can decide.
//!
//! **Harness approvals** are what this part carries. The harness asks
//! (`approval.requested`); the engine keeps the request as an approval and
//! shows it in the timeline as an `approval` item, and the thread waits
//! (`waiting_approval`). Whoever may manage the thread decides; the first
//! decision to reach the database wins, because leaving `pending` is a
//! compare-and-set, and the decision is then sent to the harness as
//! `approval.resolve` by a command that is made once however often it is
//! asked for. If the harness withdraws the request, or its turn ends first,
//! the approval is cancelled.
//!
//! Any request the harness still raises waits for its people in every mode,
//! unless an explicit matching thread rule already authorizes that action.
//! Orch never chooses an allowing option because of the mode. Questions and
//! forms always ask people for content. Harness approvals live while it waits.
//!
//! The capability kind shares the table and is raised and decided by the part
//! that owns it. A request for access is an approval too: [`access`] makes
//! one for each thing a person asks for, sends it to the nearest people who
//! can give it, carries out a decision as the approver's own change of the
//! page, and lets a request nobody answered run out.

use crate::commands::{Outcome, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::ids;
use crate::unit::Unit;
use orch_access_contract::decisions::{Decision, Refusal};
use orch_access_contract::permissions::Permission;
use orch_api_contract::approvals::{
    ApprovalDecision, ElicitationAction, HarnessRequest, HarnessRequestDetail, OptionStyle,
};
use orch_authz::engine::Graph;
use orch_authz::{Act, Caller, Principal, nodes};
use orch_db::queries::approvals::{self as queries, ApprovalRow, NewHarnessApproval};
use orch_db::queries::threads::{self, ThreadRow, thread_state};
use orch_link_contract::commands::{ApprovalResolve, SerialKey};
use orch_link_contract::events::{ApprovalClosed, ApprovalRequested};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use std::collections::BTreeSet;

/// How many times the decision on one approval is handed to the harness
/// before the engine gives the approval up as cancelled.
const RESOLVE_ATTEMPTS: u32 = 2;

/// The canonical source outcome, derived from its approval record without storing a second state.
pub(crate) async fn source_status(
    conn: &mut orch_db::Conn,
    row: &ApprovalRow,
) -> Result<Option<orch_api_contract::approvals::RequestDecisionStatus>, EngineError> {
    use orch_api_contract::approvals::{
        ApprovalState, RequestDecisionAction, RequestDecisionStatus,
    };
    let Some(at) = row.decided_at else {
        return Ok(None);
    };
    let action = match row.kind.as_str() {
        "input" if row.state == "approved" => Some(RequestDecisionAction::Run),
        "plan" => match row
            .decision
            .as_ref()
            .and_then(|decision| decision.get("option_id"))
            .and_then(Json::as_str)
        {
            Some("proceed") => Some(RequestDecisionAction::Proceed),
            Some("keep_planning") => Some(RequestDecisionAction::KeepPlanning),
            _ => None,
        },
        _ => None,
    };
    let by = match row.decided_by {
        Some(by) => Some(serde_json::from_value(
            crate::views::user_ref_json(conn, by).await?,
        )?),
        None => None,
    };
    let state: ApprovalState = serde_json::from_value(json!(row.state))?;
    Ok(Some(RequestDecisionStatus {
        action,
        state,
        decided_by: by,
        decided_at: Time::new(at),
    }))
}

impl Engine {
    /// Cancels an obsolete wait through its sole decision row, in the source operation's transaction.
    pub(crate) async fn cancel_source_request(
        &self,
        unit: &mut Unit<'_>,
        source: orch_db::queries::notifications::inbox::RequestSource,
        by: Option<Id>,
    ) -> Result<(), EngineError> {
        use orch_db::queries::notifications::inbox::RequestSource;
        let linked = match source {
            RequestSource::Approval(id) => Some(id),
            RequestSource::Input(id) => threads::input(&mut *unit.conn, id)
                .await?
                .and_then(|input| input.approval_id),
            RequestSource::Plan(id) => threads::item(&mut *unit.conn, id).await?.and_then(|item| {
                item.payload
                    .get("approval_id")
                    .and_then(Json::as_str)
                    .and_then(|id| id.parse::<Id>().ok())
            }),
        };
        let Some(id) = linked else {
            return Ok(());
        };
        let Some(row) = queries::approval(&mut *unit.conn, id).await? else {
            return Ok(());
        };
        if !matches!(row.kind.as_str(), "plan" | "input") {
            return Err(EngineError::unreadable(
                "a source points at a different request kind",
            ));
        }
        let Some(thread) = row.thread_id else {
            return Ok(());
        };
        let now = unit.now();
        if queries::decide(
            &mut *unit.conn,
            id,
            "cancelled",
            by,
            &json!({"reason":"source_no_longer_waiting"}),
            now,
        )
        .await?
        {
            let seq = unit.stamp(thread).await?;
            if row.kind == "plan"
                && let Some(item) = row.object_id
            {
                sqlx::query("UPDATE items SET seq_updated=$2,updated_at=$3 WHERE id=$1")
                    .bind(item)
                    .bind(seq)
                    .bind(now)
                    .execute(&mut *unit.conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            }
            let permission = if row.kind == "plan" {
                deciding_permission(&mut *unit.conn, thread, &json!({"request_kind":"plan"}))
                    .await?
            } else {
                Permission::ThreadInteract
            };
            let people = orch_authz::approvals::permission_deciders(
                &mut *unit.conn,
                orch_access_contract::decisions::ObjectRef {
                    kind: orch_access_contract::permissions::ObjectKind::Thread,
                    id: thread,
                },
                permission,
                Time::new(now),
            )
            .await?;
            unit.approvals_changed(&people);
        }
        Ok(())
    }

    /// Announces backfilled waits and closes obsolete source requests through the ordinary shared lifecycle.
    pub(crate) async fn reconcile_source_requests(&self) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        let rows =
            queries::source_reconciliation(&mut *unit.conn, self.inner.config.sweep_batch).await?;
        for row in rows {
            let Some(thread) = row.thread_id else {
                continue;
            };
            let Some(owner) = threads::lock(&mut *unit.conn, thread).await? else {
                continue;
            };
            let source = if row.kind == "plan" {
                orch_db::queries::notifications::inbox::RequestSource::Plan(
                    row.object_id
                        .ok_or_else(|| EngineError::unreadable("a plan has no source"))?,
                )
            } else {
                orch_db::queries::notifications::inbox::RequestSource::Input(
                    row.object_id
                        .ok_or_else(|| EngineError::unreadable("a message wait has no source"))?,
                )
            };
            let open = if row.kind == "input" {
                threads::input(
                    &mut *unit.conn,
                    row.object_id
                        .ok_or_else(|| EngineError::unreadable("a model wait has no source"))?,
                )
                .await?
                .is_some_and(|input| {
                    input.state == "pending"
                        && input.state_detail.as_deref() == Some(threads::HELD_MODEL_USE)
                        && input.approval_id == Some(row.id)
                })
            } else {
                sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM items i JOIN turns t ON t.id=i.turn_id WHERE i.id=$1 AND i.payload->>'approval_id'=$2 AND t.plan AND t.state='completed' AND NOT EXISTS(SELECT 1 FROM turns later WHERE later.thread_id=i.thread_id AND later.continuation_id=i.continuation_id AND later.ordinal>t.ordinal) AND NOT EXISTS(SELECT 1 FROM thread_inputs q WHERE q.thread_id=i.thread_id AND q.continuation_id=i.continuation_id AND q.state IN ('pending','delivering')))").bind(row.object_id).bind(row.id.to_string()).fetch_one(&mut *unit.conn).await.map_err(orch_db::DbError::from)?
            };
            if open {
                Box::pin(self.source_requested(&mut unit, &owner, source, row.turn_id)).await?;
            } else {
                Box::pin(self.cancel_source_request(
                    &mut unit,
                    orch_db::queries::notifications::inbox::RequestSource::Approval(row.id),
                    None,
                ))
                .await?;
            }
        }
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// Rechecks a source request's current thread permission, source identity and model use.
    pub async fn may_decide_source_request(
        &self,
        conn: &mut orch_db::Conn,
        principal: &Principal,
        row: &ApprovalRow,
    ) -> Result<bool, EngineError> {
        let Some(thread) = row.thread_id else {
            return Ok(false);
        };
        let Some(owner) = threads::thread(conn, thread).await? else {
            return Ok(false);
        };
        if owner.archived_at.is_some() {
            return Ok(false);
        }
        let permission = if row.kind == "plan" {
            deciding_permission(conn, thread, &json!({"request_kind":"plan"})).await?
        } else {
            Permission::ThreadInteract
        };
        if !nodes::check(
            conn,
            principal,
            thread,
            permission,
            Act::Use,
            orch_authz::Policy::default(),
        )
        .await?
        .is_allowed()
        {
            return Ok(false);
        }
        match row.kind.as_str() {
            "input" => {
                let Some(input) = (match row.object_id {
                    Some(id) => threads::input(conn, id).await?,
                    None => None,
                }) else {
                    return Ok(false);
                };
                if input.continuation_id != owner.active_continuation_id
                    || input.approval_id != Some(row.id)
                    || input.state != "pending"
                    || input.state_detail.as_deref() != Some(threads::HELD_MODEL_USE)
                {
                    return Ok(false);
                }
                let mut unit = Unit::new(conn, principal.now().get());
                Ok(self
                    .check_sender(&mut unit, principal.caller(), &owner, true)
                    .await
                    .is_ok())
            }
            "plan" => {
                let Some(item) = (match row.object_id {
                    Some(id) => threads::item(conn, id).await?,
                    None => None,
                }) else {
                    return Ok(false);
                };
                let linked = item
                    .payload
                    .get("approval_id")
                    .and_then(Json::as_str)
                    .and_then(|id| id.parse::<Id>().ok());
                Ok(linked == Some(row.id)
                    && item.continuation_id == owner.active_continuation_id
                    && owner.current_turn_id.is_none()
                    && threads::count_pending_inputs(conn, thread).await? == 0
                    && threads::latest_active_turn(conn, thread)
                        .await?
                        .is_some_and(|turn| Some(turn.id) == row.turn_id))
            }
            _ => Ok(false),
        }
    }

    /// Materializes a source wait in the same ledger as every other approval and links its source.
    pub(crate) async fn source_requested(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        source: orch_db::queries::notifications::inbox::RequestSource,
        turn: Option<Id>,
    ) -> Result<(), EngineError> {
        use orch_db::queries::notifications::inbox::RequestSource;
        // Source effects and recipient copies take locks in this order, so a completed request cannot gain an unread copy.
        let Some(current) = threads::lock(&mut *unit.conn, thread.node_id).await? else {
            return Ok(());
        };
        let thread = &current;
        if thread.archived_at.is_some() {
            return Ok(());
        }
        let now = unit.now();
        let (id, kind, key, title, permission, request, requester, linked) = match source {
            RequestSource::Plan(id) => {
                let item = threads::item(&mut *unit.conn, id)
                    .await?
                    .ok_or_else(|| EngineError::unreadable("a proposed plan has no source"))?;
                if item.continuation_id != thread.active_continuation_id
                    || thread.current_turn_id.is_some()
                    || threads::count_pending_inputs(&mut *unit.conn, thread.node_id).await? > 0
                    || !threads::latest_active_turn(&mut *unit.conn, thread.node_id)
                        .await?
                        .is_some_and(|turn| Some(turn.id) == item.turn_id)
                {
                    return Ok(());
                }
                let linked = item
                    .payload
                    .get("approval_id")
                    .and_then(Json::as_str)
                    .and_then(|id| id.parse::<Id>().ok());
                let mut plan = item.payload.clone();
                if let Some(object) = plan.as_object_mut() {
                    object.remove("approval_id");
                    object.remove("decision");
                }
                (
                    id,
                    "plan",
                    "plan_item_id",
                    "A proposed plan needs an answer",
                    deciding_permission(
                        &mut *unit.conn,
                        thread.node_id,
                        &json!({"request_kind":"plan"}),
                    )
                    .await?,
                    json!({"item_id":id,"plan":plan}),
                    None,
                    linked,
                )
            }
            RequestSource::Input(id) => {
                let input = threads::input(&mut *unit.conn, id)
                    .await?
                    .ok_or_else(|| EngineError::unreadable("a model-use wait has no input"))?;
                if input.continuation_id != thread.active_continuation_id
                    || input.state != "pending"
                    || input.state_detail.as_deref() != Some(threads::HELD_MODEL_USE)
                {
                    return Ok(());
                }
                (
                    id,
                    "input",
                    "input_id",
                    "A message waits for someone who may use the model",
                    Permission::ThreadInteract,
                    json!({"input_id":id,"revision":input.seq_updated,"content":input.content}),
                    input.author_id,
                    input.approval_id,
                )
            }
            RequestSource::Approval(_) => return Ok(()),
        };
        let existing = match linked {
            Some(linked) => queries::approval(&mut *unit.conn, linked).await?,
            None => None,
        };
        if existing.as_ref().is_some_and(|row| {
            row.kind != kind || row.object_id != Some(id) || row.thread_id != Some(thread.node_id)
        }) {
            return Err(EngineError::unreadable(
                "a source points at a different request",
            ));
        }
        if kind == "plan" && existing.as_ref().is_some_and(|row| row.state != "pending") {
            return Ok(());
        }
        let same = existing.as_ref().is_some_and(|row| {
            row.state == "pending"
                && (kind == "plan" || row.request.get("content") == request.get("content"))
        });
        let row = if same {
            existing.ok_or_else(|| EngineError::unreadable("a source lost its pending record"))?
        } else {
            if let Some(old) = existing.filter(|row| row.state == "pending") {
                queries::cancel_pending(&mut *unit.conn, old.id, now).await?;
                crate::notifications::approval_settled(unit, old.id).await?;
            }
            queries::insert_source(
                &mut *unit.conn,
                &queries::NewSourceApproval {
                    id: new_id(),
                    kind,
                    thread: thread.node_id,
                    source: id,
                    turn,
                    requester,
                    request: &request,
                    permission: permission.as_str(),
                    title,
                    at: now,
                },
            )
            .await?
        };
        let Some(row) = queries::lock(&mut *unit.conn, row.id).await? else {
            return Ok(());
        };
        if row.state != "pending" {
            return Ok(());
        }
        if linked != Some(row.id) {
            let seq = unit.stamp(thread.node_id).await?;
            match source {
                RequestSource::Plan(_) => {
                    sqlx::query("UPDATE items SET payload=jsonb_set(payload,'{approval_id}',to_jsonb($2::uuid)),seq_updated=$3,updated_at=$4 WHERE id=$1").bind(id).bind(row.id).bind(seq).bind(now).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
                }
                RequestSource::Input(_) => {
                    sqlx::query(
                        "UPDATE thread_inputs SET approval_id=$2,seq_updated=$3 WHERE id=$1",
                    )
                    .bind(id)
                    .bind(row.id)
                    .bind(seq)
                    .execute(&mut *unit.conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
                }
                RequestSource::Approval(_) => {}
            }
        }
        let recipients = self
            .approval_recipients(&mut *unit.conn, &row, Time::new(now))
            .await?;
        let mut data = json!({"approval_id":row.id,"request_kind":kind,"path":"/approvals"});
        data[key] = json!(id);
        let event = crate::notifications::Event {
            kind: crate::notifications::Kind::ApprovalRequested,
            node: Some(thread.node_id),
            thread: Some(thread.node_id),
            approval: Some(row.id),
            title: title.to_owned(),
            body: String::new(),
            data,
            source: row.id.to_string(),
            actor: None,
            at: now,
        };
        crate::notifications::tell(&mut *unit.conn, &event, &recipients, Some(thread.org_id))
            .await?;
        unit.approvals_changed(&recipients);
        Ok(())
    }

    /// Claims a source's canonical pending row before any effect is applied in the same transaction.
    pub(crate) async fn claim_source(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        source: orch_db::queries::notifications::inbox::RequestSource,
        by: Id,
        decision: &Json,
    ) -> Result<orch_api_contract::approvals::RequestDecisionStatus, crate::inputs::InputError>
    {
        use orch_db::queries::notifications::inbox::RequestSource;
        let turn = match source {
            RequestSource::Plan(id) => threads::item(&mut *unit.conn, id)
                .await?
                .and_then(|row| row.turn_id),
            _ => None,
        };
        self.source_requested(unit, thread, source, turn).await?;
        let linked = match source {
            RequestSource::Plan(id) => threads::item(&mut *unit.conn, id).await?.and_then(|item| {
                item.payload
                    .get("approval_id")
                    .and_then(Json::as_str)
                    .and_then(|id| id.parse::<Id>().ok())
            }),
            RequestSource::Input(id) => threads::input(&mut *unit.conn, id)
                .await?
                .and_then(|input| input.approval_id),
            RequestSource::Approval(id) => Some(id),
        }
        .ok_or(crate::inputs::InputError::NotFound)?;
        let row = queries::approval(&mut *unit.conn, linked)
            .await?
            .ok_or(crate::inputs::InputError::NotFound)?;
        if row.state != "pending" {
            return Err(match source_status(&mut *unit.conn, &row).await? {
                Some(status) => crate::inputs::InputError::AlreadyDecided(Box::new(status)),
                None => crate::inputs::InputError::NotPending { state: row.state },
            });
        }
        let state = if decision.get("option_id").and_then(Json::as_str) == Some("keep_planning") {
            "answered"
        } else {
            "approved"
        };
        let now = unit.now();
        if !queries::decide(&mut *unit.conn, row.id, state, Some(by), decision, now).await? {
            let current = queries::approval(&mut *unit.conn, row.id)
                .await?
                .ok_or(crate::inputs::InputError::NotFound)?;
            return Err(match source_status(&mut *unit.conn, &current).await? {
                Some(status) => crate::inputs::InputError::AlreadyDecided(Box::new(status)),
                None => crate::inputs::InputError::NotPending {
                    state: current.state,
                },
            });
        }
        let latest = queries::approval(&mut *unit.conn, row.id)
            .await?
            .ok_or(crate::inputs::InputError::NotFound)?;
        let permission = if row.kind == "plan" {
            deciding_permission(
                &mut *unit.conn,
                thread.node_id,
                &json!({"request_kind":"plan"}),
            )
            .await?
        } else {
            Permission::ThreadInteract
        };
        let readers = orch_authz::approvals::permission_deciders(
            &mut *unit.conn,
            orch_access_contract::decisions::ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Thread,
                id: thread.node_id,
            },
            permission,
            Time::new(now),
        )
        .await?;
        unit.approvals_changed(&readers);
        source_status(&mut *unit.conn, &latest)
            .await?
            .ok_or_else(|| crate::inputs::InputError::Invalid("a decision has no outcome"))
    }
}

/// Why a decision could not be taken.
#[derive(Debug, thiserror::Error)]
pub enum ApprovalError {
    /// There is no such approval.
    #[error("there is no such approval")]
    NotFound,
    /// Somebody decided it first, or it was cancelled or withdrawn.
    #[error("the approval is not pending any more")]
    AlreadyDecided,
    /// The caller may not answer what the agent of this thread asks.
    #[error("the caller may not decide this approval")]
    Denied(Box<Refusal>),
    /// The decision is not one the request can have.
    #[error("{0}")]
    Invalid(String),
    /// Something the engine stands on failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<orch_db::DbError> for ApprovalError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_authz::Error> for ApprovalError {
    fn from(error: orch_authz::Error) -> Self {
        Self::Internal(error.into())
    }
}

/// What a decision comes to for the approval and for the harness.
struct Settled {
    state: &'static str,
    for_harness: Json,
}

/// What a decision on a request means: whether it approves, and what the
/// harness is sent.
fn settle(request: &HarnessRequest, decision: &ApprovalDecision) -> Result<Settled, ApprovalError> {
    match (&request.detail, decision) {
        (HarnessRequestDetail::UserInput(input), ApprovalDecision::Answers(answers)) => {
            let ids: BTreeSet<_> = input
                .questions
                .iter()
                .map(|question| question.id.as_str())
                .collect();
            let valid = !input.questions.is_empty()
                && ids.len() == input.questions.len()
                && answers.answers.len() == ids.len()
                && answers.answers.keys().all(|id| ids.contains(id.as_str()))
                && input.questions.iter().all(|question| {
                    let Some(values) = answers.answers.get(&question.id) else {
                        return false;
                    };
                    !values.is_empty()
                        && (question.multiple || values.len() == 1)
                        && values.iter().collect::<BTreeSet<_>>().len() == values.len()
                        && values.iter().all(|value| {
                            !value.trim().is_empty()
                                && (question.allow_other
                                    || question.options.as_ref().is_none_or(|options| {
                                        options.is_empty()
                                            || options.iter().any(|option| option.label == *value)
                                    }))
                        })
                });
            if !valid {
                return Err(ApprovalError::Invalid(
                    "answer every question using its offered choices or permitted free text"
                        .to_owned(),
                ));
            }
            Ok(Settled {
                state: "answered",
                for_harness: serde_json::to_value(answers).map_err(EngineError::from)?,
            })
        }
        (HarnessRequestDetail::McpElicitation(_), ApprovalDecision::Elicitation(elicitation)) => {
            Ok(Settled {
                state: if elicitation.action == ElicitationAction::Accept {
                    "answered"
                } else {
                    "denied"
                },
                for_harness: serde_json::to_value(elicitation).map_err(EngineError::from)?,
            })
        }
        (
            HarnessRequestDetail::Command(_)
            | HarnessRequestDetail::FileChange(_)
            | HarnessRequestDetail::Permissions(_)
            | HarnessRequestDetail::Plan(_)
            | HarnessRequestDetail::Other(_),
            ApprovalDecision::Option(chosen),
        ) => {
            let option = request
                .options
                .iter()
                .find(|option| option.id == chosen.option_id)
                .ok_or_else(|| {
                    ApprovalError::Invalid(format!(
                        "the request offers no option {:?}",
                        chosen.option_id
                    ))
                })?;
            Ok(Settled {
                state: if option.style == OptionStyle::Allow {
                    "approved"
                } else {
                    "denied"
                },
                for_harness: serde_json::to_value(chosen).map_err(EngineError::from)?,
            })
        }
        _ => Err(ApprovalError::Invalid(
            "that decision does not fit what the harness asked".to_owned(),
        )),
    }
}

/// The request as the wire has it: the harness's, if it reads as one, else
/// kept whole as a request of no kind of its own.
fn request_of(requested: &ApprovalRequested) -> Result<(Json, HarnessRequest), EngineError> {
    let raw = serde_json::to_value(&requested.request)?;
    if let Ok(request) = serde_json::from_value::<HarnessRequest>(raw.clone()) {
        return Ok((raw, request));
    }
    let options = raw.get("options").cloned().unwrap_or_else(|| json!([]));
    let wrapped = json!({
        "request_kind": "other",
        "detail": { "summary": requested.title, "raw": raw },
        "options": options,
    });
    let request = serde_json::from_value::<HarnessRequest>(wrapped.clone()).unwrap_or_else(|_| {
        HarnessRequest {
            detail: HarnessRequestDetail::Other(orch_api_contract::approvals::OtherRequestDetail {
                summary: requested.title.clone(),
                raw: raw.clone(),
            }),
            options: Vec::new(),
        }
    });
    let stored = serde_json::to_value(&request)?;
    Ok((stored, request))
}

impl Engine {
    /// `approval.requested`: the harness asks. Raised once for a request
    /// however often the event is applied.
    pub(crate) async fn on_approval_requested(
        &self,
        unit: &mut Unit<'_>,
        thread_id: Id,
        segment_id: Id,
        _machine: Id,
        requested: &ApprovalRequested,
    ) -> Result<(), EngineError> {
        let Some(turn) = threads::turn(&mut *unit.conn, requested.turn_id)
            .await?
            .filter(|turn| turn.thread_id == thread_id)
        else {
            tracing::warn!(%thread_id, "an approval for a turn the thread does not have");
            return Ok(());
        };
        if !turn.is_active() {
            return Ok(());
        }
        if turn.state == threads::turn_state::STARTING {
            self.begin_turn_for(unit, thread_id, &turn).await?;
        }
        let (stored, request) = request_of(requested)?;
        let now = unit.now();
        let Some(row) = queries::insert_harness_approval(
            &mut *unit.conn,
            &NewHarnessApproval {
                id: new_id(),
                thread_id,
                turn_id: turn.id,
                native_ref: &requested.request_ref,
                title: &requested.title,
                request: &stored,
                at: now,
            },
        )
        .await?
        else {
            return Ok(());
        };
        // A rule allowed for this thread answers, once, a request that offers the very matcher it holds.
        let by_rule = match threads::thread(&mut *unit.conn, thread_id).await? {
            Some(thread) => crate::approval_rules::answer_by_rule(unit, &thread, &request).await?,
            None => None,
        };
        let status = if by_rule.is_some() {
            "approved"
        } else {
            "pending"
        };
        let parent = match requested.request.detail.native_item_ref() {
            Some(reference) => threads::item_by_native_ref(&mut *unit.conn, segment_id, reference)
                .await?
                .filter(|item| {
                    item.thread_id == thread_id
                        && item.turn_id == Some(turn.id)
                        && item.kind == "tool_call"
                })
                .map(|item| item.id),
            None => None,
        };
        let item_payload = json!({ "approval_id": row.id });
        let key = format!("approval:{}", requested.request_ref);
        let item = crate::ingest::add_item(
            unit,
            &crate::ingest::ItemSpec {
                thread: thread_id,
                segment: segment_id,
                turn: Some(turn.id),
                stream: "main",
                parent,
                kind: "approval",
                status,
                author: None,
                payload: &item_payload,
                native_ref: Some(&key),
            },
        )
        .await?;
        if let Some(item) = &item {
            queries::set_item(&mut *unit.conn, row.id, item.id).await?;
        }
        if let Some((rule, option)) = &by_rule {
            let decision = json!({ "option_id": option });
            if queries::decide(&mut *unit.conn, row.id, "approved", None, &decision, now).await? {
                let mut entry = orch_db::audit::Entry::new(
                    orch_db::audit::Actor::System,
                    orch_db::audit::name!("approval.decided"),
                    "thread",
                    Some(thread_id),
                    "An approval was answered by an action allowed for this thread",
                );
                entry.node_id = Some(thread_id);
                entry.details = json!({"approval":row.id,"turn":turn.id,"via":"rule","rule":rule,"decision":"approved"});
                orch_db::audit::append(
                    &mut *unit.conn,
                    &entry,
                    &orch_db::audit::Origin::default(),
                    now,
                )
                .await?;
            }
            unit.wake(thread_id);
            return Ok(());
        }
        let seq = unit.stamp(thread_id).await?;
        threads::set_turn_waiting(&mut *unit.conn, thread_id, true, now, seq).await?;
        threads::set_state(
            &mut *unit.conn,
            thread_id,
            &[thread_state::RUNNING, thread_state::STARTING],
            &threads::StateChange::plain(thread_state::WAITING_APPROVAL),
            seq,
        )
        .await?;
        crate::notifications::approval_asked(unit, thread_id, &row).await?;
        // Boxed, as everything added to the handling of an event: it is polled deep in the link's stack.
        let permission = Box::pin(deciding_permission(
            &mut *unit.conn,
            thread_id,
            &row.request,
        ))
        .await?;
        if let Ok(deciders) = Box::pin(self.deciders_holding_in(unit, thread_id, permission)).await
        {
            unit.approvals_changed(&deciders);
        }
        Ok(())
    }

    /// `approval.closed`: the harness withdrew a request.
    pub(crate) async fn on_approval_closed(
        &self,
        unit: &mut Unit<'_>,
        thread_id: Id,
        closed: &ApprovalClosed,
    ) -> Result<(), EngineError> {
        let Some(row) =
            queries::harness_approval_by_ref(&mut *unit.conn, thread_id, &closed.request_ref)
                .await?
        else {
            return Ok(());
        };
        let now = unit.now();
        if !queries::cancel_pending(&mut *unit.conn, row.id, now).await? {
            return Ok(());
        }
        crate::notifications::approval_settled(unit, row.id).await?;
        let seq = unit.stamp(thread_id).await?;
        if let Some(item) = row.item_id {
            threads::set_item_status(&mut *unit.conn, item, &["pending"], "cancelled", now, seq)
                .await?;
        }
        self.after_approval_left_pending(unit, thread_id).await
    }

    /// When no approval is waiting any more, the turn goes on.
    pub(crate) async fn after_approval_left_pending(
        &self,
        unit: &mut Unit<'_>,
        thread_id: Id,
    ) -> Result<(), EngineError> {
        if queries::count_pending_of_thread(&mut *unit.conn, thread_id).await? == 0 {
            let seq = unit.stamp(thread_id).await?;
            let now = unit.now();
            threads::set_turn_waiting(&mut *unit.conn, thread_id, false, now, seq).await?;
            threads::set_state(
                &mut *unit.conn,
                thread_id,
                &[thread_state::WAITING_APPROVAL],
                &threads::StateChange::plain(thread_state::RUNNING),
                seq,
            )
            .await?;
        }
        if let Ok(deciders) = self.eligible_deciders_in(unit, thread_id).await {
            unit.approvals_changed(&deciders);
        }
        if let Ok(deciders) = self
            .deciders_holding_in(unit, thread_id, Permission::ThreadInteract)
            .await
        {
            unit.approvals_changed(&deciders);
        }
        Ok(())
    }

    /// The people who may decide what the agent of a thread asks, nearest
    /// first: those who hold `thread.manage` on it.
    async fn eligible_deciders_in(
        &self,
        unit: &mut Unit<'_>,
        thread_id: Id,
    ) -> Result<Vec<Id>, EngineError> {
        Box::pin(self.deciders_holding_in(unit, thread_id, Permission::ThreadManage)).await
    }

    /// The people who hold `permission` on a thread, nearest first: those who may decide a request of it.
    async fn deciders_holding_in(
        &self,
        unit: &mut Unit<'_>,
        thread_id: Id,
        permission: Permission,
    ) -> Result<Vec<Id>, EngineError> {
        let now = Time::new(unit.now());
        Ok(orch_authz::approvals::permission_deciders(
            &mut *unit.conn,
            orch_access_contract::decisions::ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Thread,
                id: thread_id,
            },
            permission,
            now,
        )
        .await?)
    }

    /// The people who may decide what the agent of a thread asks.
    pub async fn eligible_deciders(&self, thread_id: Id) -> Result<Vec<Id>, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let mut unit = Unit::new(&mut conn, self.inner.clock.now());
        self.eligible_deciders_in(&mut unit, thread_id).await
    }

    /// Refreshes affected readers' approvals without creating a notification or unread item.
    pub fn notify_approval_changes(&self, people: &[Id]) {
        self.inner.live.approvals_changed(people);
    }

    /// The directly named owners who are told of a pending request, using its own deciding authority.
    pub async fn approval_recipients(
        &self,
        conn: &mut orch_db::Conn,
        row: &orch_db::queries::approvals::ApprovalRow,
        at: Time,
    ) -> Result<Vec<Id>, EngineError> {
        use orch_access_contract::decisions::ObjectRef;
        use orch_access_contract::permissions::ObjectKind;
        let (object, eligible) = match row.kind.as_str() {
            "harness" | "plan" | "input" => {
                let Some(thread) = row.thread_id else {
                    return Ok(Vec::new());
                };
                let permission = if row.kind == "input" {
                    Permission::ThreadInteract
                } else if row.kind == "plan" {
                    deciding_permission(conn, thread, &json!({"request_kind":"plan"})).await?
                } else {
                    deciding_permission(conn, thread, &row.request).await?
                };
                let mut unit = Unit::new(conn, at.get());
                let mut people = self
                    .deciders_holding_in(&mut unit, thread, permission)
                    .await?;
                if row.kind == "input" {
                    let owner = threads::thread(&mut *unit.conn, thread)
                        .await?
                        .ok_or_else(|| EngineError::unreadable("a request lost its thread"))?;
                    let mut filtered = Vec::new();
                    for person in people {
                        if self
                            .check_sender(
                                &mut unit,
                                &Caller::session(person).with_admin_mode(true),
                                &owner,
                                true,
                            )
                            .await
                            .is_ok()
                        {
                            filtered.push(person);
                        }
                    }
                    people = filtered;
                }
                (
                    ObjectRef {
                        kind: ObjectKind::Thread,
                        id: thread,
                    },
                    people,
                )
            }
            "port" => {
                let Some(environment) = row.environment_id else {
                    return Ok(Vec::new());
                };
                let seed = row
                    .requester_user_id
                    .or(row.decided_by)
                    .or(row.requester_thread_id);
                let Some(seed) = seed else {
                    return Ok(Vec::new());
                };
                let people = crate::ports::port_deciders(conn, environment, seed, at)
                    .await
                    .map_err(|error| EngineError::unreadable(error.to_string()))?;
                (
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: environment,
                    },
                    people,
                )
            }
            "capability" => {
                let Some(environment) = row.environment_id else {
                    return Ok(Vec::new());
                };
                let scope = row.approver_scope.as_deref().unwrap_or("system");
                let people = self.capability_approvers(conn, environment, scope).await?;
                let Some(facts) = orch_db::queries::environments::capabilities::environment_facts(
                    conn,
                    environment,
                )
                .await?
                else {
                    return Ok(Vec::new());
                };
                let object = match scope {
                    "environment" => ObjectRef {
                        kind: ObjectKind::Environment,
                        id: environment,
                    },
                    "host" => match self.capability_host(conn, environment).await? {
                        Some((host, _)) => ObjectRef {
                            kind: ObjectKind::Host,
                            id: host,
                        },
                        None => return Ok(Vec::new()),
                    },
                    "org" => {
                        let graph = Graph::load(conn, &[facts.home_node_id]).await?;
                        let org = graph
                            .get(facts.home_node_id)
                            .and_then(|node| {
                                node.path.iter().rev().find_map(|id| {
                                    graph
                                        .get(*id)
                                        .filter(|node| {
                                            node.kind == orch_access_contract::roles::NodeKind::Org
                                        })
                                        .map(|node| node.id)
                                })
                            })
                            .unwrap_or(facts.org_id);
                        ObjectRef {
                            kind: ObjectKind::Org,
                            id: org,
                        }
                    }
                    _ => {
                        if let Some(turn) = row.turn_id
                            && let Some(person) = queries::turn_starter(conn, turn).await?
                            && people.contains(&person)
                        {
                            return Ok(vec![person]);
                        }
                        return Ok(people);
                    }
                };
                (object, people)
            }
            _ => return Ok(Vec::new()),
        };
        Ok(
            orch_authz::approvals::request_recipients(conn, object, &eligible, row.turn_id, at)
                .await?,
        )
    }

    /// Decides an approval of a harness for a person who may manage its
    /// thread. The first decision to reach the database wins; the harness is
    /// told after the commit, by a command that is sent once.
    pub async fn decide_approval(
        &self,
        caller: &Caller,
        approval_id: Id,
        decision: &ApprovalDecision,
    ) -> Result<ApprovalRow, ApprovalError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let principal = Principal::load(&mut tx, caller.clone(), Time::new(now)).await?;
        self.decide_approval_guarded(tx, principal, approval_id, decision)
            .await
    }

    /// Decides as [`Self::decide_approval`] does, in a transaction whose caller was asked again once
    /// the request had come (the API's action guard): `principal` was loaded in `tx` by that check
    /// before any thread was locked, and its caller and its time are the ones that decide.
    pub async fn decide_approval_guarded(
        &self,
        mut tx: orch_db::Tx,
        principal: Principal,
        approval_id: Id,
        decision: &ApprovalDecision,
    ) -> Result<ApprovalRow, ApprovalError> {
        let caller = principal.caller().clone();
        let now = principal.now().get();
        let (finished, row) = {
            let mut unit = Unit::new(&mut tx, now);
            let approval = queries::approval(&mut *unit.conn, approval_id)
                .await?
                .filter(|row| row.kind == "harness")
                .ok_or(ApprovalError::NotFound)?;
            let thread_id = approval.thread_id.ok_or(ApprovalError::NotFound)?;
            let thread: ThreadRow = threads::lock(&mut *unit.conn, thread_id)
                .await?
                .ok_or(ApprovalError::NotFound)?;
            let permission = Box::pin(deciding_permission(
                &mut *unit.conn,
                thread_id,
                &approval.request,
            ))
            .await?;
            match nodes::check(
                &mut *unit.conn,
                &principal,
                thread_id,
                permission,
                Act::Use,
                orch_authz::Policy::default(),
            )
            .await?
            {
                Decision::Allowed { .. } => {}
                Decision::Refused(refusal) => return Err(ApprovalError::Denied(Box::new(refusal))),
            }
            if approval.state != "pending" {
                return Err(ApprovalError::AlreadyDecided);
            }
            let request: HarnessRequest = serde_json::from_value(approval.request.clone())
                .map_err(|error| {
                    EngineError::unreadable(format!("a stored request does not read: {error}"))
                })?;
            let settled = settle(&request, decision)?;
            let stored = serde_json::to_value(decision).map_err(EngineError::from)?;
            if !queries::decide(
                &mut *unit.conn,
                approval.id,
                settled.state,
                Some(caller.user()),
                &stored,
                now,
            )
            .await?
            {
                return Err(ApprovalError::AlreadyDecided);
            }
            crate::notifications::approval_settled(&mut unit, approval.id).await?;
            // Allowing for this thread keeps the rule the option showed, in this transaction.
            if let ApprovalDecision::Option(chosen) = decision
                && let Some(option) = request
                    .options
                    .iter()
                    .find(|option| option.id == chosen.option_id)
            {
                crate::approval_rules::keep(&mut unit, &thread, approval.id, option, caller.user())
                    .await?;
            }
            let _ = settled.for_harness;
            crate::audit_facts::person(
                &mut *unit.conn,
                caller.user(),
                thread_id,
                "approval.decided",
                serde_json::json!({"approval":approval.id,"decision":settled.state}),
                now,
            )
            .await?;
            let seq = unit.stamp(thread_id).await?;
            if let Some(item) = approval.item_id {
                threads::set_item_status(
                    &mut *unit.conn,
                    item,
                    &["pending"],
                    settled.state,
                    now,
                    seq,
                )
                .await?;
            }
            self.after_approval_left_pending(&mut unit, thread_id)
                .await?;
            unit.wake(thread_id);
            let _ = thread;
            let row = queries::approval(&mut *unit.conn, approval.id)
                .await?
                .ok_or(ApprovalError::NotFound)?;
            (unit.finish().await?, row)
        };
        self.commit(tx, finished).await?;
        Ok(row)
    }

    /// The threads whose decided approvals the harness has not yet been told
    /// of, or the answer to which is not yet known to have been received.
    pub(crate) async fn threads_with_decisions_to_deliver(&self) -> Result<Vec<Id>, EngineError> {
        let since = crate::config::decisions_since(self.inner.clock.now());
        let rows = {
            let mut conn = self.inner.db.acquire().await?;
            queries::decided_since(&mut conn, since, self.inner.config.sweep_batch).await?
        };
        let mut due = Vec::new();
        for row in rows {
            let Some(thread) = row.thread_id else {
                continue;
            };
            let mut settled = false;
            for attempt in 0..RESOLVE_ATTEMPTS {
                if let Some(info) = self
                    .inner
                    .commands
                    .status(ids::approval_resolve(row.id, attempt))
                    .await?
                    && matches!(info.outcome, Some(Outcome::Succeeded(_)))
                {
                    settled = true;
                }
            }
            if !settled && row.state != "cancelled" && !due.contains(&thread) {
                due.push(thread);
            }
        }
        Ok(due)
    }

    /// Hands the decisions that were taken to the harness, and settles an
    /// approval whose answer could not be given.
    pub(crate) async fn drive_approvals(&self, thread_id: Id) -> Result<(), EngineError> {
        let since = crate::config::decisions_since(self.inner.clock.now());
        let rows = {
            let mut conn = self.inner.db.acquire().await?;
            queries::decided_since(&mut conn, since, self.inner.config.sweep_batch).await?
        };
        for row in rows
            .into_iter()
            .filter(|row| row.thread_id == Some(thread_id) && row.state != "cancelled")
        {
            self.deliver_decision(thread_id, &row).await?;
        }
        Ok(())
    }

    async fn deliver_decision(&self, thread_id: Id, row: &ApprovalRow) -> Result<(), EngineError> {
        let (Some(request_ref), Some(decision)) = (row.native_ref.clone(), row.decision.clone())
        else {
            return Ok(());
        };
        let machine = {
            let mut conn = self.inner.db.acquire().await?;
            self.harness_machine_for(&mut conn, thread_id).await?
        };
        let Some(machine) = machine else {
            return Ok(());
        };
        for attempt in 0..RESOLVE_ATTEMPTS {
            let id = ids::approval_resolve(row.id, attempt);
            if self.inner.commands.status(id).await?.is_none() {
                let body = ApprovalResolve {
                    thread_id,
                    request_ref: request_ref.clone(),
                    decision: serde_json::from_value(decision.clone())?,
                };
                self.inner
                    .commands
                    .issue(Outgoing::new(
                        id,
                        machine,
                        SerialKey::Thread(thread_id),
                        &body,
                    )?)
                    .await?;
            }
            match self.inner.commands.wait(id).await? {
                Outcome::Succeeded(_) => return Ok(()),
                Outcome::Failed(ref failure) if !failure.retryable => break,
                Outcome::Cancelled => break,
                _ => {}
            }
        }
        self.give_up_decision(thread_id, row).await
    }

    /// The harness could not be told what was decided: the approval is
    /// cancelled and the timeline says why.
    async fn give_up_decision(&self, thread_id: Id, row: &ApprovalRow) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            if threads::lock(&mut *unit.conn, thread_id).await?.is_none() {
                return Ok(());
            }
            let now = unit.now();
            if queries::cancel_undelivered(&mut *unit.conn, row.id, now).await? {
                let seq = unit.stamp(thread_id).await?;
                if let Some(item) = row.item_id {
                    threads::set_item_status(
                        &mut *unit.conn,
                        item,
                        &["approved", "answered", "denied"],
                        "cancelled",
                        now,
                        seq,
                    )
                    .await?;
                }
                if let Some(turn) = row.turn_id
                    && let Some(turn) = threads::turn(&mut *unit.conn, turn).await?
                {
                    crate::ingest::add_error_item(
                            &mut unit,
                            thread_id,
                            turn.segment_id,
                            Some(turn.id),
                            "approval_undelivered",
                            "The decision could not be handed to the agent, so the request was cancelled.",
                            false,
                        )
                        .await?;
                }
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// The machine where the harness of a thread runs.
    pub(crate) async fn harness_machine_for(
        &self,
        conn: &mut orch_db::Conn,
        thread_id: Id,
    ) -> Result<Option<Id>, EngineError> {
        let Some(thread) = threads::thread(conn, thread_id).await? else {
            return Ok(None);
        };
        let (Some(provider_id), Some(environment_id)) = (thread.provider_id, thread.environment_id)
        else {
            return Ok(None);
        };
        let provider =
            orch_db::queries::resources::thread_engine::provider(conn, provider_id).await?;
        let environment =
            orch_db::queries::environments::thread_engine::environment(conn, environment_id)
                .await?;
        Ok(match (provider, environment) {
            (Some(provider), Some(environment)) => crate::harnesses::machine_of(
                thread.arrangement.as_deref(),
                &provider,
                environment.machine_id,
            ),
            _ => None,
        })
    }
}

// ---------------------------------------------------------------- requests for access
//
// One block, kept together here: an approval of the kind `access` is a person's request for permissions on a
// thing, one for each thing, sent to the nearest people who can give it. The rules are the access engine's
// (`orch_authz::requests`); what is done here is what must happen together in one transaction: the approvals
// and the line of the audit trail, the entries of the page and the end of the approval, and the people told.

/// Requests for access: made, decided, and run out.
pub mod access {
    use super::{ApprovalError, EngineError};
    use crate::engine::Engine;
    use crate::notifications::{self, Event, Kind};
    use crate::views;
    use chrono::{DateTime, Duration, Utc};
    use orch_access_contract::audit::AuditAction;
    use orch_access_contract::decisions::{Decision, ObjectRef, Refusal};
    use orch_access_contract::permissions::Permission;
    use orch_authz::pages::ChangeError;
    use orch_authz::requests::{self, Ask, AskError, Asked, Giving, GrantError, Sent, Whom};
    use orch_authz::usable::ResourcePolicy;
    use orch_authz::{Caller, Policy, Principal};
    use orch_db::audit::{self, Actor, Entry, Origin};
    use orch_db::queries::access as access_rows;
    use orch_db::queries::approvals::{self as approval_rows, ApprovalRow, access as rows};
    use orch_db::queries::hierarchy;
    use orch_db::queries::notifications::inbox;
    use orch_db::queries::threads;
    use orch_settings::{Place as SettingsPlace, get};
    use orch_settings_contract::registry::keys;
    use orch_types::{Id, Json, Time, new_id};
    use serde_json::json;
    use std::collections::HashMap;

    /// How many days a request waits for an answer when no setting says.
    pub(crate) const EXPIRY_DAYS: i64 = 14;
    /// How many batches of requests one person may have waiting when no setting says.
    pub(crate) const MOST_WAITING: i64 = 10;
    /// How many requests that ran out are let lapse in one look.
    const LAPSE_BATCH: i64 = 100;

    /// Why a request for access could not be made.
    #[derive(Debug, thiserror::Error)]
    pub enum RaiseError {
        /// Something the engine stands on failed.
        #[error(transparent)]
        Internal(#[from] EngineError),
        /// One of the things asked for cannot be asked for.
        #[error("item {index}: {error}")]
        Item {
            /// Its place in the request.
            index: usize,
            /// Why.
            error: AskError,
        },
        /// The person has as many requests waiting as the policy allows.
        #[error("{waiting} requests are waiting, and {limit} is the most")]
        Waiting {
            /// The most there may be.
            limit: i64,
            /// How many are waiting now, a batch counting once.
            waiting: i64,
        },
        /// The thread the request names is no thread.
        #[error("there is no such thread")]
        NoSuchThread,
        /// An earlier request for this thing is already waiting.
        #[error("a request for this thing is already waiting")]
        AlreadyWaiting {
            /// The existing request.
            approval: Id,
        },
    }

    impl From<orch_db::DbError> for RaiseError {
        fn from(error: orch_db::DbError) -> Self {
            Self::Internal(error.into())
        }
    }

    impl From<orch_authz::Error> for RaiseError {
        fn from(error: orch_authz::Error) -> Self {
            Self::Internal(error.into())
        }
    }

    /// The requests that were made.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Raised {
        /// The batch they were sent together as.
        pub batch_id: Id,
        /// An approval for each thing asked for, in the order they were asked.
        pub approvals: Vec<ApprovalRow>,
    }

    /// Why a request for access could not be decided as it was.
    #[derive(Debug, thiserror::Error)]
    pub enum DecideError {
        /// Something the engine stands on failed.
        #[error(transparent)]
        Internal(#[from] EngineError),
        /// There is no such request.
        #[error("there is no such request")]
        NotFound,
        /// It was decided, taken back or run out already.
        #[error("the request is not waiting any more")]
        AlreadyDecided,
        /// The person is not among those who may decide it.
        #[error("the caller may not decide this request")]
        Denied(Box<Refusal>),
        /// The decision does not fit the request.
        #[error("{path}: {message}")]
        Invalid {
            /// The member of the decision.
            path: &'static str,
            /// What is wrong.
            message: String,
        },
        /// The change of the page that approving comes to was refused.
        #[error("the change of the page was refused")]
        Change(Box<ChangeError>),
        /// Nobody decides their own request.
        #[error("a request is not decided by the person who made it")]
        OwnRequest,
    }

    impl From<orch_db::DbError> for DecideError {
        fn from(error: orch_db::DbError) -> Self {
            Self::Internal(error.into())
        }
    }

    impl From<orch_authz::Error> for DecideError {
        fn from(error: orch_authz::Error) -> Self {
            Self::Internal(error.into())
        }
    }

    impl From<ApprovalError> for DecideError {
        fn from(error: ApprovalError) -> Self {
            match error {
                ApprovalError::NotFound => Self::NotFound,
                ApprovalError::AlreadyDecided => Self::AlreadyDecided,
                ApprovalError::Denied(refusal) => Self::Denied(refusal),
                ApprovalError::Invalid(message) => Self::Invalid { path: "", message },
                ApprovalError::Internal(error) => Self::Internal(error),
            }
        }
    }

    /// What a person decides on a request.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Decided {
        /// Whether they give it.
        pub approve: bool,
        /// The permissions to give, if fewer than were asked.
        pub permissions: Option<Vec<String>>,
        /// Narrower options.
        pub options: Option<Json>,
        /// When what is given ends.
        pub expires_at: Option<Time>,
        /// A note for whoever asked.
        pub note: Option<String>,
    }

    /// The settings that decide a request about a thing at `place`: whether its owners may name
    /// somebody from outside, and which subscriptions its home shares.
    async fn policy_at(
        conn: &mut orch_db::Conn,
        place: Option<Id>,
    ) -> Result<ResourcePolicy, EngineError> {
        let Some(place) = place else {
            return Ok(ResourcePolicy {
                owners_name_outsiders: true,
                shared_subscriptions: None,
            });
        };
        let allowed: Option<bool> = get(
            conn,
            SettingsPlace::Node(place),
            keys::SHARING_ALLOW_EXTERNAL,
        )
        .await?;
        let shared: Option<Vec<String>> = get(
            conn,
            SettingsPlace::Node(place),
            keys::PROVIDERS_SUBSCRIPTION_SHARING,
        )
        .await?;
        Ok(ResourcePolicy {
            owners_name_outsiders: allowed.unwrap_or(true),
            shared_subscriptions: shared,
        })
    }

    /// The words of the permissions of an ask, for a line that shows it.
    fn labels(permissions: &[Permission]) -> String {
        let mut words: Vec<&str> = permissions.iter().map(|one| one.label()).collect();
        words.dedup();
        words.join(", ")
    }

    fn day(span: i64) -> Duration {
        Duration::days(span)
    }

    fn expiry(now: DateTime<Utc>, days: i64) -> DateTime<Utc> {
        now + day(days)
    }

    impl Engine {
        /// Asks for access to things on behalf of `caller`, one approval for each: each is sent to
        /// the nearest people who can give that thing, and they are told, in the transaction that
        /// makes the request. A request is refused whole when one of its things cannot be asked
        /// for, and when the person has as many batches waiting as the policy allows.
        pub async fn raise_access_requests(
            &self,
            caller: &Caller,
            asks: &[Ask],
            reason: &str,
            for_thread: Option<Id>,
            origin: &Origin,
        ) -> Result<Raised, RaiseError> {
            let now = self.inner.clock.now();
            let mut tx = self.inner.db.begin().await?;
            let principal = Principal::load(&mut tx, caller.clone(), Time::new(now)).await?;
            self.raise_access_requests_guarded(tx, principal, asks, reason, for_thread, origin)
                .await
        }

        /// Asks as [`Self::raise_access_requests`] does, in a transaction whose caller was asked again
        /// once the request had come (the API's action guard): `principal` was loaded in `tx` by that
        /// check, and its caller and its time are the ones the request is made with.
        pub async fn raise_access_requests_guarded(
            &self,
            mut tx: orch_db::Tx,
            principal: Principal,
            asks: &[Ask],
            reason: &str,
            for_thread: Option<Id>,
            origin: &Origin,
        ) -> Result<Raised, RaiseError> {
            let caller = principal.caller().clone();
            let caller = &caller;
            let now = principal.now().get();
            rows::lock_requester(&mut tx, caller.user()).await?;
            let mut objects = std::collections::HashSet::new();
            for (index, ask) in asks.iter().enumerate() {
                if !objects.insert(ask.object.id) {
                    return Err(RaiseError::Item {
                        index,
                        error: AskError::Invalid {
                            path: "object",
                            message: "is repeated in this batch".to_owned(),
                        },
                    });
                }
            }
            for ask in asks {
                if let Some(waiting) =
                    rows::pending_for_person(&mut tx, caller.user(), ask.object.id, now).await?
                {
                    return Err(RaiseError::AlreadyWaiting {
                        approval: waiting.id,
                    });
                }
            }
            let most: i64 = get(
                &mut tx,
                SettingsPlace::System,
                keys::ACCESS_MAX_PENDING_REQUESTS,
            )
            .await
            .map_err(EngineError::from)?
            .unwrap_or(MOST_WAITING);
            let waiting = rows::waiting_batches(&mut tx, caller.user(), now).await?;
            if waiting >= most {
                return Err(RaiseError::Waiting {
                    limit: most,
                    waiting,
                });
            }
            let days: i64 = get(
                &mut tx,
                SettingsPlace::System,
                keys::ACCESS_REQUEST_EXPIRY_DAYS,
            )
            .await
            .map_err(EngineError::from)?
            .unwrap_or(EXPIRY_DAYS);
            if let Some(thread) = for_thread
                && threads::thread(&mut tx, thread)
                    .await
                    .map_err(EngineError::from)?
                    .is_none()
            {
                return Err(RaiseError::NoSuchThread);
            }
            let who = access_rows::user_names(&mut tx, &[caller.user()])
                .await?
                .into_iter()
                .next()
                .map_or_else(|| "Somebody".to_owned(), |named| named.name);
            // What the request keeps is what the API shows of it, so that names are kept as they
            // were when it was asked.
            let requester_ref = views::user_ref_json(&mut tx, caller.user()).await?;
            let thread_ref = match for_thread {
                Some(thread) => node_ref_json(&mut tx, thread).await?,
                None => Json::Null,
            };
            let batch = new_id();
            let mut made: Vec<(ApprovalRow, Sent, Vec<Permission>)> =
                Vec::with_capacity(asks.len());
            for (index, ask) in asks.iter().enumerate() {
                let place = requests::summary(&mut tx, ask.object)
                    .await
                    .map_err(|error| RaiseError::Item { index, error })?
                    .place;
                let policy = policy_at(&mut tx, place).await?;
                let sent = requests::plan(&mut tx, &principal, ask, &policy)
                    .await
                    .map_err(|error| RaiseError::Item { index, error })?;
                let asked = Asked::of(ask, reason);
                let holder = match ask.whom {
                    Whom::Requester => json!({ "kind": "user", "user": requester_ref }),
                    Whom::Person { id } => {
                        json!({ "kind": "user", "user": views::user_ref_json(&mut tx, id).await? })
                    }
                    Whom::Work { id } => {
                        json!({ "kind": "node", "node": node_ref_json(&mut tx, id).await? })
                    }
                };
                let stored = asked.to_json(holder, thread_ref.clone());
                let is_node = sent.node.is_some();
                let title = format!(
                    "{who} asks for {} on {}",
                    labels(&ask.permissions),
                    sent.name
                );
                let row = rows::insert(
                    &mut tx,
                    &rows::NewAccessApproval {
                        id: new_id(),
                        batch_id: batch,
                        node_id: sent.node.or(sent.beside),
                        object_kind: (!is_node).then(|| sent.kind.as_str()),
                        object_id: (!is_node).then_some(sent.id),
                        thread_id: for_thread,
                        requester: caller.user(),
                        title: &title,
                        request: &stored,
                        required_permission: rows::REQUIRED_PERMISSION,
                        expires_at: expiry(now, days),
                        at: now,
                    },
                )
                .await?;
                made.push((row, sent, ask.permissions.clone()));
            }
            let summary = format!("{who} asked for access to {} things", made.len());
            let mut entry = Entry::new(
                Actor::User(caller.user()),
                orch_db::audit::action!(AuditAction::AccessRequested).as_str(),
                "access",
                Some(batch),
                &summary,
            );
            entry.org_id = made.iter().find_map(|(_, sent, _)| sent.root);
            entry.details = json!({
                "batch_id": batch,
                "reason": reason,
                "items": made.iter().map(|(row, sent, permissions)| json!({
                    "approval": row.id,
                    "object": { "kind": sent.kind.as_str(), "id": sent.id },
                    "permissions": permissions.iter().map(|one| one.as_str()).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            });
            audit::append(&mut tx, &entry, origin, now)
                .await
                .map_err(EngineError::from)?;
            let mut deciders: Vec<Id> = Vec::new();
            for (row, sent, permissions) in &made {
                let event = Event {
                    kind: Kind::AccessRequested,
                    node: sent.node,
                    thread: for_thread,
                    approval: Some(row.id),
                    title: format!("{who} asks for access to {}", sent.name),
                    body: format!("{}: {reason}", labels(permissions)),
                    data: json!({
                        "approval_id": row.id,
                        "batch_id": batch,
                        "permissions": permissions.iter().map(|permission| permission.as_str()).collect::<Vec<_>>(),
                        "reason": reason,
                        "path": "/approvals",
                    }),
                    source: row.id.to_string(),
                    actor: Some(caller.user()),
                    at: now,
                };
                let eligible: Vec<_> = sent
                    .givers
                    .nearest
                    .iter()
                    .chain(&sent.givers.further)
                    .copied()
                    .collect();
                let people = orch_authz::approvals::owners_to_tell(
                    &mut tx,
                    orch_access_contract::decisions::ObjectRef {
                        kind: sent.kind,
                        id: sent.id,
                    },
                    &eligible,
                    Time::new(now),
                )
                .await?;
                notifications::tell(&mut tx, &event, &people, sent.root).await?;
                deciders.extend(sent.givers.nearest.iter().chain(&sent.givers.further));
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
            deciders.sort_unstable();
            deciders.dedup();
            self.inner.live.approvals_changed(&deciders);
            Ok(Raised {
                batch_id: batch,
                approvals: made.into_iter().map(|(row, _, _)| row).collect(),
            })
        }

        /// Whether a person may decide the request `row` and whether it was sent to them.
        pub async fn standing_on_access_request(
            &self,
            caller: &Caller,
            row: &ApprovalRow,
        ) -> Result<requests::Standing, EngineError> {
            let mut conn = self.inner.db.acquire().await?;
            if crate::membership_requests::is_join(row) {
                let principal =
                    Principal::load(&mut conn, caller.clone(), Time::new(self.inner.clock.now()))
                        .await?;
                return self
                    .access_decision_authority(&mut conn, &principal, row)
                    .await;
            }
            let Some((object, asked, requester)) = read_request(&mut conn, row).await? else {
                return Ok(requests::Standing::default());
            };
            let principal =
                Principal::load(&mut conn, caller.clone(), Time::new(self.inner.clock.now()))
                    .await?;
            Ok(requests::standing(
                &mut conn,
                &principal,
                object,
                &asked.permissions,
                requester,
                Policy::default(),
            )
            .await?)
        }

        /// Current boundary authority, in the caller's guarded connection and at its sampled time.
        pub async fn access_decision_authority(
            &self,
            conn: &mut orch_db::Conn,
            principal: &Principal,
            row: &ApprovalRow,
        ) -> Result<requests::Standing, EngineError> {
            if crate::membership_requests::is_join(row) {
                let may_decide = match (row.object_id, row.requester_user_id) {
                    (Some(chat), Some(person)) => {
                        crate::membership_requests::may_add(conn, principal, chat, person).await?
                    }
                    _ => false,
                };
                return Ok(requests::Standing {
                    may_decide,
                    can_give: may_decide,
                    ..requests::Standing::default()
                });
            }
            let Some((object, asked, requester)) = read_request(conn, row).await? else {
                return Ok(requests::Standing::default());
            };
            Ok(requests::standing(
                conn,
                principal,
                object,
                &asked.permissions,
                requester,
                Policy::default(),
            )
            .await?)
        }

        /// Decides a request for access for a person who may change the page of what it asks for.
        /// Approving names the holder asked for in the rows asked for, or in fewer, as the
        /// approver's own change of the page, in the same transaction; denying gives nothing. The
        /// first decision to reach the database wins; the person who asked is told.
        pub async fn decide_access_request(
            &self,
            caller: &Caller,
            approval_id: Id,
            decided: &Decided,
            origin: &Origin,
        ) -> Result<ApprovalRow, DecideError> {
            let now = self.inner.clock.now();
            let mut tx = self.inner.db.begin().await?;
            let approver = Principal::load(&mut tx, caller.clone(), Time::new(now)).await?;
            self.decide_access_request_guarded(tx, approver, approval_id, decided, origin)
                .await
        }

        /// Decides as [`Self::decide_access_request`] does, in a transaction whose caller was asked
        /// again once the request had come (the API's action guard): `approver` was loaded in `tx` by
        /// that check, and its caller and its time are the ones the decision is made with.
        pub async fn decide_access_request_guarded(
            &self,
            mut tx: orch_db::Tx,
            approver: Principal,
            approval_id: Id,
            decided: &Decided,
            origin: &Origin,
        ) -> Result<ApprovalRow, DecideError> {
            let caller = approver.caller().clone();
            let caller = &caller;
            let now = approver.now().get();
            // A request that ran out is not decided; it is let lapse at once, and that is all this
            // transaction does.
            let overdue = approval_rows::approval(&mut tx, approval_id)
                .await?
                .filter(|row| row.kind == "access" && row.state == "pending")
                .is_some_and(|row| row.expires_at.is_some_and(|end| end <= now));
            if overdue {
                rows::expire(&mut tx, approval_id, now).await?;
                inbox::settle_approval(&mut tx, approval_id, now).await?;
                tx.commit().await.map_err(orch_db::DbError::from)?;
                return Err(DecideError::AlreadyDecided);
            }
            let row = approval_rows::approval(&mut tx, approval_id)
                .await?
                .filter(|row| row.kind == "access")
                .ok_or(DecideError::NotFound)?;
            if row.state != "pending" {
                return Err(DecideError::AlreadyDecided);
            }
            if crate::membership_requests::is_join(&row) {
                return self
                    .decide_join_guarded(tx, approver, row, decided, origin)
                    .await;
            }
            let (object, asked, requester) =
                read_request(&mut tx, &row).await?.ok_or_else(|| {
                    DecideError::Internal(EngineError::unreadable("a stored request does not read"))
                })?;
            if approver.user() == requester {
                return Err(DecideError::OwnRequest);
            }
            let place = requests::summary(&mut tx, object)
                .await
                .map_err(|_| DecideError::NotFound)?
                .place;
            let policy = policy_at(&mut tx, place).await?;
            // Whoever decides is somebody who could change the page.
            let givers = requests::givers_for(
                &mut tx,
                &approver,
                object,
                &asked.permissions,
                None,
                &policy,
            )
            .await
            .map_err(|error| EngineError::unreadable(error.to_string()))?;
            let mut readers: Vec<_> = givers
                .nearest
                .into_iter()
                .chain(givers.further)
                .chain([requester, caller.user()])
                .collect();
            readers.sort_unstable();
            readers.dedup();
            if let Decision::Refused(refusal) =
                requests::may_decide(&mut tx, &approver, object, policy.node_policy()).await?
            {
                return Err(DecideError::Denied(Box::new(refusal)));
            }
            let giving = if decided.approve {
                Some(giving_of(decided)?)
            } else {
                None
            };
            let state = if decided.approve {
                "approved"
            } else {
                "denied"
            };
            let stored = json!({
                "approve": decided.approve,
                "permissions": decided.permissions,
                "options": decided.options,
                "expires_at": decided.expires_at,
                "note": decided.note,
            });
            if !approval_rows::decide(&mut tx, row.id, state, Some(caller.user()), &stored, now)
                .await?
            {
                return Err(DecideError::AlreadyDecided);
            }
            let mut given: Vec<Permission> = Vec::new();
            if let Some(giving) = &giving {
                let (_, permissions) = requests::grant(
                    &mut tx, &approver, requester, object, &asked, giving, &policy, origin,
                )
                .await
                .map_err(|error| match error {
                    GrantError::Engine(error) => DecideError::Internal(error.into()),
                    GrantError::Invalid { path, message } => DecideError::Invalid { path, message },
                    GrantError::Change(error) => DecideError::Change(error),
                    GrantError::OwnRequest => DecideError::OwnRequest,
                })?;
                given = permissions;
            }
            let approver_name = access_rows::user_names(&mut tx, &[caller.user()])
                .await?
                .into_iter()
                .next()
                .map_or_else(|| "Somebody".to_owned(), |named| named.name);
            let site = requests::summary(&mut tx, object)
                .await
                .map_err(|_| DecideError::NotFound)?;
            let action = if decided.approve {
                orch_db::audit::action!(AuditAction::AccessApproved)
            } else {
                orch_db::audit::action!(AuditAction::AccessDenied)
            };
            let summary = format!(
                "{approver_name} {} a request for access to {}",
                if decided.approve {
                    "approved"
                } else {
                    "denied"
                },
                site.name
            );
            let mut entry = Entry::new(
                Actor::User(caller.user()),
                action.as_str(),
                "access",
                Some(row.id),
                &summary,
            );
            entry.node_id = row.node_id;
            entry.org_id = site.root;
            entry.details = json!({
                "approval": row.id,
                "batch_id": row.batch_id,
                "requester": requester,
                "object": { "kind": object.kind.as_str(), "id": object.id },
                "asked": asked.permissions.iter().map(|one| one.as_str()).collect::<Vec<_>>(),
                "given": given.iter().map(|one| one.as_str()).collect::<Vec<_>>(),
                "note": decided.note,
            });
            audit::append(&mut tx, &entry, origin, now)
                .await
                .map_err(EngineError::from)?;
            // Nobody has anything left to do about it: what asked them to decide is read.
            inbox::settle_approval(&mut tx, row.id, now).await?;
            let verdict = if decided.approve {
                "approved"
            } else {
                "declined"
            };
            let note = decided.note.clone().unwrap_or_default();
            let requester_principal =
                Principal::load(&mut tx, Caller::session(requester), Time::new(now)).await;
            let readable = match requester_principal {
                Ok(principal) => {
                    requests::named_for(&mut tx, &principal, object, Policy::default())
                        .await?
                        .is_some()
                }
                Err(_) => false,
            };
            let shared_name = if readable {
                site.name.clone()
            } else {
                asked
                    .label
                    .clone()
                    .unwrap_or_else(|| object.kind.as_str().to_owned())
            };
            let event = Event {
                kind: Kind::AccessDecided,
                node: if readable { row.node_id } else { None },
                thread: row.thread_id,
                approval: Some(row.id),
                title: format!(
                    "{approver_name} {verdict} your request for access to {}",
                    shared_name
                ),
                body: if decided.approve {
                    format!("{} was given.", labels(&given))
                } else {
                    note.clone()
                },
                data: json!({
                    "approval_id": row.id,
                    "batch_id": row.batch_id,
                    "approved": decided.approve,
                    "permissions": given.iter().map(|permission| permission.as_str()).collect::<Vec<_>>(),
                    "note": note,
                    "path": "/approvals",
                }),
                source: format!("{}:{verdict}", row.id),
                actor: Some(caller.user()),
                at: now,
            };
            notifications::tell(&mut tx, &event, &[requester], site.root).await?;
            let everything = decided.approve
                && self
                    .batch_is_held(&mut tx, requester, row.batch_id, now)
                    .await?;
            if everything {
                let done = Event {
                    kind: Kind::AccessDecided,
                    node: row.node_id,
                    thread: row.thread_id,
                    approval: None,
                    title: "Everything you asked for is yours".to_owned(),
                    body: String::new(),
                    data: json!({ "batch_id": row.batch_id, "complete": true, "path": "/approvals" }),
                    source: format!("batch:{}:complete", row.batch_id.unwrap_or(row.id)),
                    actor: Some(caller.user()),
                    at: now,
                };
                notifications::tell(&mut tx, &done, &[requester], site.root).await?;
            }
            let decided_row = approval_rows::approval(&mut tx, row.id)
                .await?
                .ok_or(DecideError::NotFound)?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            self.inner.live.approvals_changed(&readers);
            Ok(decided_row)
        }

        /// Whether everything a batch asked for is held now, by whatever means.
        async fn batch_is_held(
            &self,
            conn: &mut orch_db::Conn,
            requester: Id,
            batch: Option<Id>,
            now: DateTime<Utc>,
        ) -> Result<bool, EngineError> {
            let Some(batch) = batch else {
                return Ok(false);
            };
            let principal =
                Principal::load(conn, Caller::session(requester), Time::new(now)).await?;
            for row in rows::batch(conn, batch).await? {
                if crate::membership_requests::is_join(&row) {
                    if !crate::membership_requests::is_member(
                        conn,
                        row.object_id
                            .ok_or_else(|| EngineError::unreadable("a join request has no chat"))?,
                        requester,
                        Time::new(now),
                    )
                    .await?
                    {
                        return Ok(false);
                    }
                    continue;
                }
                let Some((object, asked, _)) = read_request(conn, &row).await? else {
                    return Ok(false);
                };
                if !requests::is_held(conn, &principal, object, &asked, Policy::default()).await? {
                    return Ok(false);
                }
            }
            Ok(true)
        }

        /// Whether everything a batch asked for is held now, for the person who asked.
        pub async fn access_batch_is_held(
            &self,
            requester: Id,
            batch: Id,
        ) -> Result<bool, EngineError> {
            let mut conn = self.inner.db.acquire().await?;
            self.batch_is_held(&mut conn, requester, Some(batch), self.inner.clock.now())
                .await
        }

        /// Lets the requests for access that nobody decided in time lapse. Called by the
        /// supervisor.
        pub async fn expire_access_requests(&self) -> Result<(), EngineError> {
            let now = self.inner.clock.now();
            let due = {
                let mut conn = self.inner.db.acquire().await?;
                rows::overdue(&mut conn, now, LAPSE_BATCH).await?
            };
            for row in due {
                let mut tx = self.inner.db.begin().await?;
                if rows::expire(&mut tx, row.id, now).await? {
                    inbox::settle_approval(&mut tx, row.id, now).await?;
                }
                tx.commit().await.map_err(orch_db::DbError::from)?;
            }
            Ok(())
        }
    }

    /// What the stored requests of `rows` say, for each the thing it asks about, what it asks
    /// and who asked: the thing is the one the approval's own members name, and for a node its
    /// kind is read from the node, for all of them at once. A row that does not read is left out.
    pub async fn read_requests(
        conn: &mut orch_db::Conn,
        rows: &[&ApprovalRow],
    ) -> Result<HashMap<Id, (ObjectRef, Asked, Id)>, EngineError> {
        let nodes: Vec<Id> = rows
            .iter()
            .filter(|row| row.object_kind.is_none())
            .filter_map(|row| row.node_id)
            .collect();
        let kinds: HashMap<Id, orch_access_contract::permissions::ObjectKind> = if nodes.is_empty()
        {
            HashMap::new()
        } else {
            hierarchy::with_ancestors(conn, &nodes)
                .await?
                .into_iter()
                .filter_map(|found| Some((found.id, found.kind.parse().ok()?)))
                .collect()
        };
        Ok(rows
            .iter()
            .filter_map(|row| {
                let object = match (row.object_kind.as_deref(), row.object_id) {
                    (Some(kind), Some(id)) => ObjectRef {
                        kind: kind.parse().ok()?,
                        id,
                    },
                    _ => {
                        let node = row.node_id?;
                        ObjectRef {
                            kind: *kinds.get(&node)?,
                            id: node,
                        }
                    }
                };
                let asked = Asked::from_json(object, &row.request)?;
                Some((row.id, (object, asked, row.requester_user_id?)))
            })
            .collect())
    }

    /// What the stored request of `row` says: the thing it asks about, what it asks and who asked.
    pub async fn read_request(
        conn: &mut orch_db::Conn,
        row: &ApprovalRow,
    ) -> Result<Option<(ObjectRef, Asked, Id)>, EngineError> {
        Ok(read_requests(conn, &[row]).await?.remove(&row.id))
    }

    /// A node as the API names it, kept in a request: by its kind and its name as they are now.
    async fn node_ref_json(conn: &mut orch_db::Conn, id: Id) -> Result<Json, EngineError> {
        Ok(hierarchy::with_ancestors(conn, &[id])
            .await?
            .into_iter()
            .find(|node| node.id == id)
            .map_or_else(
                || json!({ "id": id, "kind": "org", "name": null, "redacted": true }),
                |node| json!({ "id": id, "kind": node.kind, "name": node.name, "redacted": false }),
            ))
    }

    /// What a decision gives, as the access engine reads it.
    fn giving_of(decided: &Decided) -> Result<Giving, DecideError> {
        let permissions = match &decided.permissions {
            None => None,
            Some(named) => Some(
                named
                    .iter()
                    .map(|name| {
                        name.parse::<Permission>()
                            .map_err(|_| DecideError::Invalid {
                                path: "permissions",
                                message: format!("{name} is no permission"),
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        };
        Ok(Giving {
            permissions,
            options: decided.options.clone(),
            expires_at: decided.expires_at,
        })
    }
}

/// What deciding a stored request of a thread takes: a plan the agent proposes is answered by whoever may write to
/// the agent there (`thread.interact`), or by its managers where the place keeps plans to them
/// (`thread.plan_answered_by`); everything else the agent asks by those who manage the thread.
pub async fn deciding_permission(
    conn: &mut orch_db::Conn,
    thread: Id,
    request: &Json,
) -> Result<Permission, EngineError> {
    if request.get("request_kind").and_then(Json::as_str) != Some("plan") {
        return Ok(Permission::ThreadManage);
    }
    // Boxed: this is asked from deep in the handling of an event and of a request.
    let by = Box::pin(orch_settings::get::<String>(
        conn,
        orch_settings::Place::Node(thread),
        orch_settings_contract::registry::keys::THREAD_PLAN_ANSWERED_BY,
    ))
    .await?;
    Ok(if by.as_deref() == Some("managers") {
        Permission::ThreadManage
    } else {
        Permission::ThreadInteract
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::approvals::{HarnessAnswersDecision, HarnessOptionDecision};
    use serde_json::json;

    struct NoMachineWork;

    #[async_trait::async_trait]
    impl crate::commands::Commands for NoMachineWork {
        async fn issue(
            &self,
            _: crate::commands::Outgoing,
        ) -> Result<(), crate::commands::CommandsError> {
            panic!("an empty source-request sweep sends no machine command")
        }

        async fn status(
            &self,
            _: Id,
        ) -> Result<Option<crate::commands::Info>, crate::commands::CommandsError> {
            panic!("an empty source-request sweep inspects no machine command")
        }

        async fn wait(
            &self,
            _: Id,
        ) -> Result<crate::commands::Outcome, crate::commands::CommandsError> {
            panic!("an empty source-request sweep waits for no machine command")
        }

        async fn cancel(&self, _: Id) -> Result<bool, crate::commands::CommandsError> {
            panic!("an empty source-request sweep cancels no machine command")
        }
    }

    #[tokio::test]
    async fn a_fresh_database_runs_the_source_request_sweep_without_machine_work() {
        let db = orch_testkit::database::TestDb::fresh().await;
        let engine = Engine::new(crate::engine::EngineParts {
            db: orch_db::Db::from_pool(db.pool.clone()),
            clock: std::sync::Arc::new(orch_types::SystemClock),
            config: crate::config::EngineConfig::default(),
            commands: std::sync::Arc::new(NoMachineWork),
            access: std::sync::Arc::new(crate::access::Unconnected),
            live: std::sync::Arc::new(crate::live::Discard),
        });
        engine.reconcile_source_requests().await.unwrap();
        let approvals: i64 = sqlx::query_scalar("SELECT count(*) FROM approvals")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(approvals, 0);
        drop(engine);
        db.close().await.unwrap();
    }

    fn command_request() -> HarnessRequest {
        serde_json::from_value(json!({
            "request_kind": "command",
            "detail": { "command": "rm -rf build", "cwd": "/work", "reason": null },
            "options": [
                { "id": "allow_once", "label": "Allow once", "style": "allow", "scope": "once" },
                { "id": "deny", "label": "Deny", "style": "deny", "scope": null }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn choosing_an_option_that_allows_approves_and_one_that_refuses_denies() {
        let allow = ApprovalDecision::Option(HarnessOptionDecision {
            option_id: "allow_once".into(),
            note: None,
        });
        assert_eq!(
            settle(&command_request(), &allow).unwrap().state,
            "approved"
        );
        let deny = ApprovalDecision::Option(HarnessOptionDecision {
            option_id: "deny".into(),
            note: Some("not now".into()),
        });
        assert_eq!(settle(&command_request(), &deny).unwrap().state, "denied");
    }

    fn plan_request() -> HarnessRequest {
        serde_json::from_value(json!({
            "request_kind": "plan",
            "detail": { "plan": "Read the files, then make the change." },
            "options": [
                { "id": "proceed", "label": "Proceed", "style": "allow", "scope": "once" },
                { "id": "keep_planning", "label": "Keep planning", "style": "deny", "scope": null }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn proceeding_with_a_harness_plan_approves_the_offered_option() {
        let decided = settle(
            &plan_request(),
            &ApprovalDecision::Option(HarnessOptionDecision {
                option_id: "proceed".into(),
                note: None,
            }),
        )
        .unwrap();
        assert_eq!(decided.state, "approved");
        assert_eq!(decided.for_harness, json!({ "option_id": "proceed" }));
    }

    #[test]
    fn keeping_a_harness_plan_denies_and_preserves_the_persons_words() {
        let decided = settle(
            &plan_request(),
            &ApprovalDecision::Option(HarnessOptionDecision {
                option_id: "keep_planning".into(),
                note: Some("Include rollback before changing anything. 请先读配置。".into()),
            }),
        )
        .unwrap();
        assert_eq!(decided.state, "denied");
        assert_eq!(
            decided.for_harness,
            json!({
                "option_id": "keep_planning",
                "note": "Include rollback before changing anything. 请先读配置。"
            })
        );
    }

    #[test]
    fn a_plan_still_refuses_an_option_it_did_not_offer() {
        assert!(matches!(
            settle(
                &plan_request(),
                &ApprovalDecision::Option(HarnessOptionDecision {
                    option_id: "allow_everything".into(),
                    note: None,
                })
            ),
            Err(ApprovalError::Invalid(_))
        ));
    }

    #[test]
    fn a_decision_that_does_not_fit_the_request_is_refused() {
        let unknown = ApprovalDecision::Option(HarnessOptionDecision {
            option_id: "grant_all".into(),
            note: None,
        });
        assert!(matches!(
            settle(&command_request(), &unknown),
            Err(ApprovalError::Invalid(_))
        ));
        let answers = ApprovalDecision::Answers(HarnessAnswersDecision {
            answers: Default::default(),
            note: None,
        });
        assert!(matches!(
            settle(&command_request(), &answers),
            Err(ApprovalError::Invalid(_))
        ));
    }

    fn question_request(
        multiple: bool,
        allow_other: bool,
        choices: Option<Vec<&str>>,
    ) -> HarnessRequest {
        serde_json::from_value(json!({
            "request_kind": "user_input", "options": [],
            "detail": { "questions": [{ "id": "choice", "header": "Choice", "question": "Which value?",
                "multiple": multiple, "allow_other": allow_other,
                "options": choices.map(|values| values.into_iter().map(|label| json!({ "label": label, "description": label })).collect::<Vec<_>>())
            }] }
        })).unwrap()
    }

    fn question_answer(values: Json) -> ApprovalDecision {
        serde_json::from_value(json!({ "answers": values })).unwrap()
    }

    #[test]
    fn invalid_question_answers_never_produce_a_settlement() {
        let request = question_request(false, false, Some(vec!["First", "Second"]));
        let invalid = [
            json!({}),
            json!({ "unknown": ["First"] }),
            json!({ "choice": [], }),
            json!({ "choice": [""] }),
            json!({ "choice": ["   "] }),
            json!({ "choice": ["Unlisted"] }),
            json!({ "choice": ["First", "Second"] }),
            json!({ "choice": ["First"], "unknown": ["Second"] }),
        ];
        let accepted: Vec<_> = invalid
            .into_iter()
            .enumerate()
            .filter_map(|(index, values)| {
                (!matches!(
                    settle(&request, &question_answer(values)),
                    Err(ApprovalError::Invalid(_))
                ))
                .then_some(index)
            })
            .collect();
        assert!(
            accepted.is_empty(),
            "invalid answer cases produced settlements: {accepted:?}"
        );
    }

    #[test]
    fn questions_end_answered_and_preserve_the_persons_exact_content() {
        for (request, values) in [
            (
                question_request(false, false, Some(vec!["First", "Second"])),
                json!({ "choice": ["First"] }),
            ),
            (
                question_request(true, false, Some(vec!["First", "Second"])),
                json!({ "choice": ["First", "Second"] }),
            ),
            (
                question_request(false, true, Some(vec!["First", "Second"])),
                json!({ "choice": ["A different value"] }),
            ),
            (
                question_request(false, false, None),
                json!({ "choice": ["My own words"] }),
            ),
        ] {
            let decision = question_answer(values.clone());
            let result = settle(&request, &decision).unwrap();
            assert_eq!(result.state, "answered");
            assert_eq!(result.for_harness, json!({ "answers": values }));
        }
    }

    #[test]
    fn duplicate_or_blank_answers_are_invalid_even_when_other_words_are_allowed() {
        let request = question_request(true, true, Some(vec!["First", "Second"]));
        for values in [
            json!({ "choice": ["First", "First"] }),
            json!({ "choice": ["First", "  "] }),
        ] {
            assert!(matches!(
                settle(&request, &question_answer(values)),
                Err(ApprovalError::Invalid(_))
            ));
        }
    }

    #[test]
    fn a_submitted_form_is_answered_while_declining_is_not_a_content_submission() {
        let request: HarnessRequest = serde_json::from_value(json!({
            "request_kind":"mcp_elicitation", "options":[],
            "detail":{"server":"form-server","message":"Choose a value","schema":{"type":"object"}}
        }))
        .unwrap();
        let accept: ApprovalDecision =
            serde_json::from_value(json!({"action":"accept","content":{"value":"chosen"}}))
                .unwrap();
        let decline: ApprovalDecision =
            serde_json::from_value(json!({"action":"decline"})).unwrap();
        let submitted = settle(&request, &accept).unwrap();
        assert_eq!(submitted.state, "answered");
        assert_eq!(
            submitted.for_harness,
            json!({"action":"accept","content":{"value":"chosen"}})
        );
        assert_eq!(settle(&request, &decline).unwrap().state, "denied");
    }

    #[test]
    fn an_other_request_keeps_the_harness_body_and_kind() {
        let requested = ApprovalRequested {
            turn_id: new_id(),
            request_ref: "r1".into(),
            title: "odd".into(),
            request: orch_link_contract::foreign::HarnessRequest {
                request_kind: orch_agent_contract::events::HarnessRequestKind::Other,
                options: Vec::new(),
                detail: orch_agent_contract::events::HarnessRequestDetail::Other {
                    summary: "odd".into(),
                    raw: json!({ "something": "else" }),
                },
            },
        };
        let (stored, request) = request_of(&requested).unwrap();
        assert!(matches!(request.detail, HarnessRequestDetail::Other(_)));
        assert_eq!(stored["request_kind"], "other");
        assert_eq!(stored["detail"]["raw"], json!({ "something": "else" }));
    }
}
