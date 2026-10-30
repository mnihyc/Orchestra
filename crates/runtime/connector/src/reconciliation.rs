//! Reopen spool and reconcile hosted sessions.
//!
//! A connector that starts again picks up where the one before left off. The link reopens the spool and
//! sends again what the system has not taken in ([`orch_link::machine::MachineLink::start`]). This module
//! is the other half: what the connector was hosting.
//!
//! The harness processes that were running are started again, with the request the system started them
//! with, in the home they had. A process cannot take its sessions with it across the connector's
//! restart, so none is open afterwards, and the system is told what became of each:
//!
//! * a session in which **a turn was running** is reported lost: that turn is over, whatever the harness
//!   kept of it;
//! * a session whose harness **no longer has it** is reported lost;
//! * any other session is left unreported. The harness still has it, nothing is running in it, and the
//!   system opens it again (`resume`) when its thread next needs it; the report of the process, which
//!   says that none is open now, is what tells it so.
//!
//! A process that cannot be started again loses all its sessions, and is reported failed.

use std::sync::Arc;

use orch_agent_contract::facts::HarnessState;
use orch_agent_contract::session::ThreadId;
use orch_link_contract::events::Event;
use orch_link_contract::streams::Stream;
use orch_types::Id;

use crate::events::harness_status;
use crate::harnesses::Hosting;
use crate::ledger::HostedSession;

pub(crate) mod environments;

/// What a session that was running a turn is told when the connector starts again.
pub const TURN_LOST: &str = "the connector stopped while a turn was running";
/// What a session that its harness no longer has is told.
pub const SESSION_GONE: &str = "the harness no longer has the session";

async fn lost(hosting: &Hosting, thread: ThreadId, segment: Id, reason: &str) {
    let event = orch_agent_contract::events::SessionEvent::SessionLost(
        orch_agent_contract::events::SessionLost {
            reason: reason.to_owned(),
        },
    );
    let body = serde_json::to_value(&event)
        .ok()
        .and_then(|written| written.get("body").cloned())
        .unwrap_or(serde_json::Value::Null);
    if let Ok(converted) = Event::decode(event.kind(), body) {
        let published = hosting
            .services()
            .publisher
            .publish(
                Stream::Session {
                    thread_id: thread,
                    segment_id: segment,
                },
                converted,
            )
            .await;
        if let Err(error) = published {
            tracing::warn!(%error, "a lost session could not be reported");
        }
    }
    hosting.ledger().session_closed(thread);
}

fn why(session: &HostedSession, held: Option<&[String]>) -> Option<&'static str> {
    if session.running_turn.is_some() {
        return Some(TURN_LOST);
    }
    match held {
        Some(held) if !held.contains(&session.native_session_ref) => Some(SESSION_GONE),
        _ => None,
    }
}

/// Takes up what the connector was hosting when it stopped.
pub async fn reconcile(hosting: &Arc<Hosting>) {
    for hosted in hosting.recorded() {
        let provider = hosted.start.provider_id;
        let agent = hosted.start.agent_id;
        match hosting.restart_recorded(&hosted).await {
            Ok(process) => {
                let held = process.native_refs().await;
                for session in &hosted.sessions {
                    match why(session, held.as_deref()) {
                        Some(reason) => {
                            lost(hosting, session.thread_id, session.segment_id, reason).await;
                        }
                        None => hosting.ledger().session_closed(session.thread_id),
                    }
                }
            }
            Err(error) => {
                let detail = format!("the harness process could not be started again: {error}");
                for session in &hosted.sessions {
                    lost(hosting, session.thread_id, session.segment_id, &detail).await;
                }
                hosting.ledger().harness_stopped(provider, agent);
                let report = orch_agent_contract::facts::HarnessStatus {
                    status: HarnessState::Error,
                    detail: Some(detail),
                    harness_version: None,
                    sessions_open: 0,
                };
                let published = hosting
                    .services()
                    .publisher
                    .publish(
                        Stream::Harness {
                            provider_id: provider,
                            agent_id: agent,
                        },
                        Event::HarnessStatus(harness_status(&report)),
                    )
                    .await;
                if let Err(error) = published {
                    tracing::warn!(%error, "a failed harness could not be reported");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    fn session(turn: Option<Id>) -> HostedSession {
        HostedSession {
            thread_id: new_id(),
            segment_id: new_id(),
            native_session_ref: "fake_1".into(),
            running_turn: turn,
        }
    }

    #[test]
    fn a_session_that_ran_a_turn_is_lost_whatever_the_harness_still_has() {
        let held = vec!["fake_1".to_owned()];
        assert_eq!(why(&session(Some(new_id())), Some(&held)), Some(TURN_LOST));
        assert_eq!(why(&session(Some(new_id())), None), Some(TURN_LOST));
    }

    #[test]
    fn an_idle_session_is_lost_only_when_the_harness_says_it_has_gone() {
        let held = vec!["fake_1".to_owned()];
        assert_eq!(why(&session(None), Some(&held)), None);
        assert_eq!(why(&session(None), Some(&[])), Some(SESSION_GONE));
        assert_eq!(
            why(&session(None), None),
            None,
            "a harness that cannot say is not taken for gone"
        );
    }
}
