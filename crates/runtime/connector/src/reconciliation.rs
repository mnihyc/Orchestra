//! Reopen spool and reconcile hosted sessions.
//!
//! A connector that starts again picks up where the one before left off. The link reopens the spool and
//! sends again what the system has not taken in ([`orch_link::machine::MachineLink::start`]). This module
//! is the other half: what the connector was hosting.
//!
//! The harness processes that were running are started again, with the request the system started them
//! with, in the home they had. Before classifying any admitted work as lost, the exact native turn is
//! inspected without submitting input. Confirmed terminal history is replayed with its admitted identity;
//! missing sessions are lost. An unrecoverable admitted turn is reported by its recorded identity. Its native
//! outcome stays unconfirmed and its input is never replayed by reconciliation.

use std::sync::Arc;

use crate::ledger::HostedSession;
use orch_agent_contract::facts::HarnessState;
use orch_link_contract::events::{Event, SessionLost, TurnLost};
use orch_link_contract::streams::Stream;

use crate::events::harness_status;
use crate::harnesses::Hosting;

pub(crate) mod environments;

/// What a session that its harness no longer has is told.
pub const SESSION_GONE: &str = "the harness no longer has the session";

async fn lost(hosting: &Hosting, session: &HostedSession, reason: &str, uncertain: bool) {
    let stream = Stream::Session {
        thread_id: session.thread_id,
        segment_id: session.segment_id,
    };
    let event = match (uncertain, session.running_turn) {
        (true, Some(turn_id)) => Event::TurnLost(TurnLost {
            turn_id,
            reason: reason.into(),
        }),
        _ => Event::SessionLost(SessionLost {
            reason: reason.into(),
            uncertain,
        }),
    };
    match hosting.services().publisher.publish(stream, event).await {
        Err(error) => tracing::warn!(%error, "a lost session could not be reported"),
        Ok(()) if !uncertain => hosting.ledger().session_closed(session.thread_id),
        Ok(()) => {}
    }
}

/// Takes up what the connector was hosting when it stopped.
pub async fn reconcile(hosting: &Arc<Hosting>) {
    for hosted in hosting.recorded() {
        let provider = hosted.start.provider_id;
        let agent = hosted.start.agent_id;
        match hosting.restart_recorded(&hosted).await {
            Ok(process) => {
                for session in &hosted.sessions {
                    match process.recover_recorded_turn(session).await {
                        Ok(true) if session.running_turn.is_none() => {
                            hosting.ledger().session_closed(session.thread_id)
                        }
                        // A recovered terminal event is still traversing the bounded event sink.
                        // Retain its exact source until a later open/close replaces it; replay is idempotent.
                        Ok(true) => {}
                        Err(error)
                            if matches!(error.code.as_str(), "not_found" | "session_not_found") =>
                        {
                            lost(hosting, session, SESSION_GONE, false).await
                        }
                        result => {
                            let detail=match result {Err(error)=>format!("Native history may remain, but recovery of the exact admitted turn is unavailable: {}",error.detail),_=>"Native history remains, but the exact admitted turn has no confirmed terminal outcome and cannot be reattached. Its input was not retried.".to_owned()};
                            lost(hosting, session, &detail, true).await;
                        }
                    }
                }
            }
            Err(error) => {
                let detail = format!("the harness process could not be started again: {error}");
                for session in &hosted.sessions {
                    lost(hosting, session, &detail, true).await;
                }
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
