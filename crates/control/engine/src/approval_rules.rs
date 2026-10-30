//! Narrow answers allowed for one thread: one rule model, kept by Orch and never written into a harness's settings.
//!
//! A request offers "Allow for this thread" only where its adapter supplies a matcher it verifies whole (an option
//! of scope `thread` with its `rule`); the matcher is shown before it is chosen. Choosing it keeps the rule in
//! `thread_approval_rules`, bound to the thread's harness, provider, profile, environment and directory, in the
//! decision's transaction. A later request that offers the same matcher in its own option of scope `thread`,
//! while the thread keeps that binding, is answered once by the rule with the request's own allowing option of
//! scope `once`; the engine compares every member of the matcher and of the binding and interprets no command.
//! Questions, forms and requests without a verifiable matcher still wait. A person who manages the thread takes a
//! rule back, serialized with deciding on the thread's lock; a change of what the thread runs on ends them all;
//! none is copied into a fork or a side thread.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::unit::Unit;
use crate::views;
use orch_access_contract::decisions::Decision;
use orch_access_contract::permissions::Permission;
use orch_api_contract::approvals::{
    ApprovalOption, HarnessRequest, HarnessRequestDetail, OptionScope, OptionStyle,
    ThreadApprovalRule,
};
use orch_authz::{Act, Principal, nodes};
use orch_db::queries::approval_rules::{self as rows, Binding};
use orch_db::queries::threads::{self, ThreadRow};
use orch_types::{Id, Time, new_id};
use serde_json::json;

/// Why a rule could not be taken back.
#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    /// There is no such thread or rule.
    #[error("there is no such rule")]
    NotFound,
    /// The caller may not manage the thread.
    #[error("the caller may not manage this thread")]
    Denied(Box<orch_access_contract::decisions::Refusal>),
    /// Something the engine stands on failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<orch_db::DbError> for RuleError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_authz::Error> for RuleError {
    fn from(error: orch_authz::Error) -> Self {
        Self::Internal(error.into())
    }
}

/// What a thread runs on now, as a rule is bound to it; none for a thread that is not bound.
pub(crate) async fn binding_of(
    conn: &mut orch_db::Conn,
    thread: &ThreadRow,
) -> Result<Option<Binding>, EngineError> {
    let (Some(agent_id), Some(provider_id), Some(environment_id)) =
        (thread.agent_id, thread.provider_id, thread.environment_id)
    else {
        return Ok(None);
    };
    let Some(agent) = orch_db::queries::resources::thread_engine::agent(conn, agent_id).await?
    else {
        return Ok(None);
    };
    Ok(Some(Binding {
        harness_kind: agent.harness_kind,
        provider_id,
        agent_id,
        environment_id,
        directory: thread.workspace_dir.clone(),
    }))
}

/// The option of a request that allows for this thread, with the matcher it shows.
fn thread_option(option: &ApprovalOption) -> Option<serde_json::Value> {
    (option.style == OptionStyle::Allow && option.scope == Some(OptionScope::Thread))
        .then_some(option.rule.as_ref())
        .flatten()
        .and_then(|rule| serde_json::to_value(rule).ok())
}

/// Rules cover a verified permission request, never a plan, question, form,
/// file change or unknown request, regardless of the options it carries.
fn permits_rule(request: &HarnessRequest) -> bool {
    matches!(
        &request.detail,
        HarnessRequestDetail::Command(_) | HarnessRequestDetail::Permissions(_)
    )
}

/// Keeps the rule a person allowed by choosing `option` of a request of `thread`, where it is an option for this
/// thread with its matcher; nothing otherwise. Its line is in the thread and its act in the audit.
pub(crate) async fn keep(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    approval: Id,
    request: &HarnessRequest,
    option: &ApprovalOption,
    by: Id,
) -> Result<Option<Id>, EngineError> {
    if !permits_rule(request) {
        return Ok(None);
    }
    let Some(matcher) = thread_option(option) else {
        return Ok(None);
    };
    let Some(binding) = binding_of(&mut *unit.conn, thread).await? else {
        return Ok(None);
    };
    let now = unit.now();
    let seq = unit.stamp(thread.node_id).await?;
    let id = new_id();
    rows::insert(
        &mut *unit.conn,
        id,
        thread.node_id,
        approval,
        &binding,
        &matcher,
        by,
        now,
        seq,
    )
    .await?;
    crate::audit_facts::person(
        &mut *unit.conn,
        by,
        thread.node_id,
        orch_db::audit::name!("approval.rule_added"),
        json!({ "rule": id, "approval": approval, "matcher": matcher }),
        now,
    )
    .await?;
    line(unit, thread, id, "allowed", Some(by)).await?;
    Ok(Some(id))
}

/// The rule that answers a request of `thread` as it arrives, with the request's own option that allows once: a
/// request that offers, in its option for this thread, a matcher an active rule of the same binding holds.
pub(crate) async fn answer_by_rule(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    request: &HarnessRequest,
) -> Result<Option<(Id, String)>, EngineError> {
    if !permits_rule(request) {
        return Ok(None);
    }
    let Some(once) = request.options.iter().find(|option| {
        option.style == OptionStyle::Allow && option.scope == Some(OptionScope::Once)
    }) else {
        return Ok(None);
    };
    let offered: Vec<serde_json::Value> =
        request.options.iter().filter_map(thread_option).collect();
    if offered.is_empty() {
        return Ok(None);
    }
    let Some(binding) = binding_of(&mut *unit.conn, thread).await? else {
        return Ok(None);
    };
    for matcher in &offered {
        if let Some(rule) =
            rows::active_matching(&mut *unit.conn, thread.node_id, &binding, matcher).await?
        {
            return Ok(Some((rule, once.id.clone())));
        }
    }
    Ok(None)
}

/// Ends every active rule of a thread whose binding changes; each says so in the thread and the audit.
pub(crate) async fn end_for_binding_change(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
) -> Result<(), EngineError> {
    let active = rows::active_of(&mut *unit.conn, thread.node_id).await?;
    if active.is_empty() {
        return Ok(());
    }
    let now = unit.now();
    for rule in active {
        let seq = unit.stamp(thread.node_id).await?;
        if rows::revoke(&mut *unit.conn, rule, None, "binding_changed", now, seq).await? {
            let mut entry = orch_db::audit::Entry::new(
                orch_db::audit::Actor::System,
                orch_db::audit::name!("approval.rule_revoked"),
                "thread",
                Some(thread.node_id),
                "An action allowed for a thread ended when what the thread runs on changed.",
            );
            entry.node_id = Some(thread.node_id);
            entry.details = json!({ "rule": rule, "reason": "binding_changed" });
            orch_db::audit::append(
                &mut *unit.conn,
                &entry,
                &orch_db::audit::Origin::default(),
                now,
            )
            .await?;
            line(unit, thread, rule, "revoked", None).await?;
        }
    }
    Ok(())
}

/// The line of a rule in its thread.
async fn line(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    rule: Id,
    action: &str,
    by: Option<Id>,
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
    let by = match by {
        Some(by) => views::user_ref_json(&mut *unit.conn, by).await?,
        None => serde_json::Value::Null,
    };
    let payload = json!({
        "marker": "approval_rules_changed",
        "data": { "rule_id": rule, "action": action, "by": by },
    });
    crate::ingest::add_marker(
        unit,
        thread.node_id,
        segment,
        None,
        &payload,
        &format!("approval_rules_changed:{rule}:{action}"),
    )
    .await
}

impl Engine {
    /// The rules of a thread, newest first, as whoever manages it sees them: page by page after `before`.
    pub async fn approval_rules(
        &self,
        principal: &Principal,
        thread: Id,
        before: Option<Id>,
        limit: i64,
    ) -> Result<Vec<ThreadApprovalRule>, RuleError> {
        let mut conn = self.inner.db.acquire().await?;
        match nodes::check(
            &mut conn,
            principal,
            thread,
            Permission::ThreadManage,
            Act::Use,
            orch_authz::Policy::default(),
        )
        .await?
        {
            Decision::Allowed { .. } => {}
            Decision::Refused(refusal) => return Err(RuleError::Denied(Box::new(refusal))),
        }
        let found = rows::list(&mut conn, thread, before, limit).await?;
        let mut shown = Vec::with_capacity(found.len());
        for row in found {
            let by = views::user_ref_json(&mut conn, row.created_by).await?;
            shown.push(ThreadApprovalRule {
                id: row.id,
                matcher: serde_json::from_value(row.matcher).map_err(EngineError::from)?,
                by: serde_json::from_value(by).map_err(EngineError::from)?,
                at: Time::new(row.created_at),
                active: row.revoked_at.is_none() && row.inactive_reason.is_none(),
                reason: row.inactive_reason,
                revision: row.revision,
            });
        }
        Ok(shown)
    }

    /// Takes back a rule of a thread for a person who manages it, in the transaction the API's action guard asked
    /// again in, under the thread's lock: the next undecided request asks again; an action already allowed goes on.
    pub async fn revoke_approval_rule_guarded(
        &self,
        mut tx: orch_db::Tx,
        principal: Principal,
        thread_id: Id,
        rule: Id,
    ) -> Result<(), RuleError> {
        let now = principal.now().get();
        let by = principal.caller().user();
        let finished = {
            let mut unit = Unit::new(&mut tx, now);
            let thread = threads::lock(&mut *unit.conn, thread_id)
                .await?
                .ok_or(RuleError::NotFound)?;
            match nodes::check(
                &mut *unit.conn,
                &principal,
                thread_id,
                Permission::ThreadManage,
                Act::Use,
                orch_authz::Policy::default(),
            )
            .await?
            {
                Decision::Allowed { .. } => {}
                Decision::Refused(refusal) => return Err(RuleError::Denied(Box::new(refusal))),
            }
            if rows::rule(&mut *unit.conn, thread_id, rule)
                .await?
                .is_none()
            {
                return Err(RuleError::NotFound);
            }
            let seq = unit.stamp(thread_id).await?;
            if rows::revoke(&mut *unit.conn, rule, Some(by), "revoked", now, seq).await? {
                crate::audit_facts::person(
                    &mut *unit.conn,
                    by,
                    thread_id,
                    "approval.rule_revoked",
                    json!({ "rule": rule }),
                    now,
                )
                .await?;
                line(&mut unit, &thread, rule, "revoked", Some(by)).await?;
            }
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }
}
