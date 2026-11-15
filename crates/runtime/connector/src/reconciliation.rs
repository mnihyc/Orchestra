//! Reopen spool and reconcile hosted sessions.
//!
//! A connector that starts again picks up where the one before left off. The link reopens the spool and
//! sends again what the system has not taken in ([`orch_link::machine::MachineLink::start`]). This module
//! is the other half: what the connector was hosting.
//!
//! The harness processes that were running are started again, with the request the system started them
//! with, in the home they had. Before classifying any admitted work as lost, the exact native turn is
//! inspected without submitting input. Confirmed terminal history is replayed with its admitted identity;
//! missing sessions are lost. Unavailable inspection retains both the ledger and the active turn, with
//! a precise visible reason, instead of retrying work whose native outcome is unknown.

use std::sync::Arc;

use orch_agent_contract::facts::HarnessState;
use orch_agent_contract::session::ThreadId;
use orch_link_contract::events::Event;
use orch_link_contract::streams::Stream;
use orch_types::Id;

use crate::events::harness_status;
use crate::harnesses::Hosting;

pub(crate) mod environments;

/// What a session that its harness no longer has is told.
pub const SESSION_GONE: &str = "the harness no longer has the session";

async fn lost(hosting: &Hosting, thread: ThreadId, segment: Id, reason: &str, uncertain: bool) {
    let event = orch_agent_contract::events::SessionEvent::SessionLost(
        orch_agent_contract::events::SessionLost {
            reason: reason.to_owned(),
            uncertain,
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
        match published {
            Err(error) => tracing::warn!(%error, "a lost session could not be reported"),
            Ok(()) if !uncertain => hosting.ledger().session_closed(thread),
            Ok(()) => {}
        }
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
                            lost(
                                hosting,
                                session.thread_id,
                                session.segment_id,
                                SESSION_GONE,
                                false,
                            )
                            .await
                        }
                        result => {
                            let detail=match result {Err(error)=>format!("Native history may remain, but recovery of the exact admitted turn is unavailable: {}",error.detail),_=>"Native history remains, but the exact admitted turn has no confirmed terminal outcome and cannot be reattached. Its input was not retried.".to_owned()};
                            lost(
                                hosting,
                                session.thread_id,
                                session.segment_id,
                                &detail,
                                true,
                            )
                            .await;
                        }
                    }
                }
            }
            Err(error) => {
                let detail = format!("the harness process could not be started again: {error}");
                for session in &hosted.sessions {
                    lost(
                        hosting,
                        session.thread_id,
                        session.segment_id,
                        &detail,
                        true,
                    )
                    .await;
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
