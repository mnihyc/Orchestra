//! Compact waiting and blocked explanations for the same authorized tree over HTTP and the live feed.

use crate::middleware::requests::ApiResult;
use crate::routes::approvals;
use orch_api_contract::approvals::{ApprovalRequest, ApprovalState, HarnessRequestKind};
use orch_api_contract::nodes::{TreeBlockedCause, TreeWaiting, TreeWaitingKind};
use orch_authz::Principal;
use orch_db::{Conn, queries::approvals as rows};
use orch_engine::{Engine, sessions};
use orch_types::Id;
use std::collections::HashMap;

/// Reuse approval listing's current authority, including capability and environment-scoped requests.
/// A thread's owner, participation, approval mode or native reviewer cannot substitute for it.
pub(crate) async fn waiting(
    conn: &mut Conn,
    principal: &Principal,
    threads: &[Id],
    at: chrono::DateTime<chrono::Utc>,
    engine: &Engine,
) -> ApiResult<HashMap<Id, TreeWaiting>> {
    let found = rows::pending_for_threads(conn, threads, at).await?;
    if found.is_empty() {
        return Ok(HashMap::new());
    }
    let mut waits: HashMap<Id, TreeWaiting> = HashMap::new();
    for approval in approvals::views(conn, principal, &found, at, engine).await? {
        if approval.state != ApprovalState::Pending {
            continue;
        }
        let Some(thread) = approval.thread_id else {
            continue;
        };
        let kind = match &approval.request {
            ApprovalRequest::Harness(request)
                if matches!(
                    request.detail.kind(),
                    HarnessRequestKind::UserInput | HarnessRequestKind::McpElicitation
                ) =>
            {
                TreeWaitingKind::Answer
            }
            _ => TreeWaitingKind::Approval,
        };
        let candidate = TreeWaiting {
            kind,
            for_caller: approval.can_decide,
        };
        waits
            .entry(thread)
            .and_modify(|held| prefer(held, candidate))
            .or_insert(candidate);
    }
    Ok(waits)
}

/// Name the reader's own outstanding request first; two eligible requests prefer the question.
fn prefer(held: &mut TreeWaiting, candidate: TreeWaiting) {
    let rank = |wait: TreeWaiting| (wait.for_caller, wait.kind == TreeWaitingKind::Answer);
    if rank(candidate) > rank(*held) {
        *held = candidate;
    }
}

/// Recognize only the engine's own setup refusals; vendor text is never parsed or sent to home.
pub(crate) fn blocked_cause(
    state: Option<&str>,
    reason: Option<&str>,
    detail: Option<&str>,
) -> Option<TreeBlockedCause> {
    if state != Some("blocked") || reason != Some("binding_invalid") {
        return None;
    }
    match detail? {
        detail if sessions::is_sandbox_refusal(detail) => Some(TreeBlockedCause::Sandbox),
        sessions::APPROVAL_NOT_ALLOWED | sessions::APPROVAL_UNSUPPORTED => {
            Some(TreeBlockedCause::ApprovalMode)
        }
        "second_step_required" => Some(TreeBlockedCause::SecondStep),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_readers_approval_is_not_replaced_by_someone_elses_question() {
        let mut shown = TreeWaiting {
            kind: TreeWaitingKind::Approval,
            for_caller: true,
        };
        prefer(
            &mut shown,
            TreeWaiting {
                kind: TreeWaitingKind::Answer,
                for_caller: false,
            },
        );
        assert_eq!(shown.kind, TreeWaitingKind::Approval);
        prefer(
            &mut shown,
            TreeWaiting {
                kind: TreeWaitingKind::Answer,
                for_caller: true,
            },
        );
        assert_eq!(
            shown,
            TreeWaiting {
                kind: TreeWaitingKind::Answer,
                for_caller: true
            }
        );
    }

    #[test]
    fn only_a_known_current_binding_refusal_names_its_setup_cause() {
        assert_eq!(
            blocked_cause(
                Some("blocked"),
                Some("binding_invalid"),
                Some(sessions::SANDBOX_UNSUPPORTED)
            ),
            Some(TreeBlockedCause::Sandbox)
        );
        assert_eq!(
            blocked_cause(
                Some("blocked"),
                Some("binding_invalid"),
                Some(sessions::APPROVAL_UNSUPPORTED)
            ),
            Some(TreeBlockedCause::ApprovalMode)
        );
        assert_eq!(
            blocked_cause(
                Some("blocked"),
                Some("binding_invalid"),
                Some("second_step_required")
            ),
            Some(TreeBlockedCause::SecondStep)
        );
        assert_eq!(
            blocked_cause(
                Some("idle"),
                Some("binding_invalid"),
                Some(sessions::SANDBOX_UNSUPPORTED)
            ),
            None
        );
        assert_eq!(
            blocked_cause(
                Some("blocked"),
                Some("error"),
                Some(sessions::SANDBOX_UNSUPPORTED)
            ),
            None
        );
        assert_eq!(
            blocked_cause(
                Some("blocked"),
                Some("binding_invalid"),
                Some("vendor says sandbox.mode is unknown")
            ),
            None
        );
    }
}
