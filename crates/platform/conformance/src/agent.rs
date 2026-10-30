//! The suite every agent adapter passes: the command and event catalogue, sessions, sign-in methods and credential forms it declares.
//!
//! The fixture names a disposable harness home, sessions and a deterministic turn. Native prompt and
//! credential proofs are separate cases; this suite never spends a subscription or starts a login by itself.

use std::time::Duration;

use orch_agent_contract::adapter::AdapterEvent;
use orch_agent_contract::adapter::{AgentAdapter, AgentStart};
use orch_agent_contract::requests::UsageRequest;
use orch_agent_contract::session::{SessionSpec, TurnStart};
use serde::{Deserialize, Serialize};

use crate::report::Checked;
use crate::report::{Report, skip};
use crate::scripted::Controller;
use orch_types::Id;
use std::collections::HashMap;

/// Checks a turn's raw request reports identically for native adapters and external JSON lines.
pub fn requests(events: &[AdapterEvent], thread: Id, segment: Id, turn: Id) -> Checked {
    let mut latest: HashMap<&str, &UsageRequest> = HashMap::new();
    for event in events {
        let AdapterEvent::ModelRequest(report) = event else {
            continue;
        };
        if report.turn_id != turn {
            continue;
        }
        if report.thread_id != thread || report.segment_id != segment {
            return Err("a model request names the wrong session".into());
        }
        let request = &report.request;
        request.check().map_err(crate::report::Miss::from)?;
        if let Some(previous) = latest.get(request.id.as_str()) {
            if previous.subagent != request.subagent {
                return Err("a model request changed its stream".into());
            }
            if previous.outcome.is_some() {
                if *previous == request {
                    continue;
                }
                return Err("a completed model request changed after its terminal report".into());
            }
            for (before, after) in [
                (previous.asked, request.asked),
                (previous.first_output, request.first_output),
                (previous.end, request.end),
            ] {
                if before.is_some() && before != after {
                    return Err("a model request changed an observed boundary".into());
                }
            }
        }
        latest.insert(&request.id, request);
    }
    if latest.is_empty() {
        return Err(
            "every turn reports its requests, even when their boundaries are not measured".into(),
        );
    }
    if latest.values().any(|request| request.outcome.is_none()) {
        return Err("every model request needs a terminal outcome".into());
    }
    Ok(())
}

/// Startup and a deterministic short turn have the ordinary adapter deadline.
const WITHIN: Duration = Duration::from_secs(120);

/// A disposable harness context and two independent sessions for the adapter's catalogue checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fixture {
    /// Startup configuration with a home belonging only to the check.
    pub start: AgentStart,
    /// Two different thread and segment ids, directories and session keys.
    pub sessions: Vec<SessionSpec>,
    /// One deterministic input for each session, using that session's thread id.
    pub turns: Vec<TurnStart>,
}

/// Checks the descriptor and, with a fixture, drives two sessions through the neutral event catalogue.
pub async fn run(adapter: &mut dyn AgentAdapter, fixture: Option<&Fixture>, report: &mut Report) {
    let Some(fixture) = fixture else {
        report.checked(
            "agent.fixture",
            "Its declared harness behavior is exercised",
            Err(skip(
                "supply an agent fixture with a disposable home and controlled model",
            )),
        );
        return;
    };
    let (sink, mut controller) = Controller::new();
    let started = tokio::time::timeout(WITHIN, adapter.start(fixture.start.clone(), sink)).await;
    let descriptor = match started {
        Ok(Ok(started)) => {
            report.checked(
                "agent.start",
                "The harness starts and reports its actual version",
                {
                    if started.harness_version.is_empty() {
                        Err("no harness version reported".into())
                    } else {
                        Ok(())
                    }
                },
            );
            Some(started.descriptor)
        }
        Ok(Err(error)) => {
            report.checked(
                "agent.start",
                "The harness starts and reports its actual version",
                Err(error.to_string().into()),
            );
            None
        }
        Err(_) => {
            report.checked(
                "agent.start",
                "The harness starts and reports its actual version",
                Err("the harness did not start before its deadline".into()),
            );
            None
        }
    };
    if let Some(descriptor) = descriptor {
        report.checked(
            "agent.descriptor",
            "Its capabilities, arrangements and forms are consistent",
            descriptor.check().map_err(|faults| {
                faults
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
                    .into()
            }),
        );
        report
            .run(
                "agent.probe",
                "A probe describes the same kind of running harness",
                WITHIN,
                async {
                    let probed = adapter.probe().await.map_err(|error| error.to_string())?;
                    if probed.kind != descriptor.kind || adapter.kind() != descriptor.kind {
                        return Err("a probe or adapter names another harness kind".into());
                    }
                    probed
                        .check()
                        .map_err(|_| "the probed descriptor is inconsistent".into())
                },
            )
            .await;
        report
            .run(
                "agent.sessions",
                "Two sessions keep distinct native identities and route their own turns",
                WITHIN,
                async {
                    if fixture.sessions.len() != 2 || fixture.turns.len() != 2 {
                        return Err(
                            "the fixture needs exactly two independent sessions and turns".into(),
                        );
                    }
                    let first = &fixture.sessions[0];
                    let second = &fixture.sessions[1];
                    if first.settings.thread_id == second.settings.thread_id
                        || first.session_key == second.session_key
                    {
                        return Err("the fixture reuses a thread id or session key".into());
                    }
                    let mut native_refs = Vec::new();
                    for (spec, turn) in fixture.sessions.iter().zip(&fixture.turns) {
                        if turn.thread_id != spec.settings.thread_id {
                            return Err("a fixture turn names a different thread".into());
                        }
                        let opened = adapter
                            .open_session(spec.clone())
                            .await
                            .map_err(|error| error.to_string())?;
                        if opened.native_session_ref.is_empty()
                            || native_refs.contains(&opened.native_session_ref)
                        {
                            return Err(
                                "independent sessions have an empty or shared native identity"
                                    .into(),
                            );
                        }
                        native_refs.push(opened.native_session_ref);
                        adapter
                            .start_turn(turn.clone())
                            .await
                            .map_err(|error| error.to_string())?;
                        controller
                            .completed(
                                turn.thread_id,
                                spec.settings.segment_id,
                                turn.turn_id,
                                WITHIN,
                            )
                            .await?;
                        requests(
                            &controller.events,
                            turn.thread_id,
                            spec.settings.segment_id,
                            turn.turn_id,
                        )?;
                        adapter
                            .close_session(spec.settings.thread_id)
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                    Ok(())
                },
            )
            .await;
        report.checked(
            "agent.requests",
            "Every request keeps ordered boundaries, nullable counts and a terminal outcome",
            fixture
                .sessions
                .iter()
                .zip(&fixture.turns)
                .try_for_each(|(session, turn)| {
                    requests(
                        &controller.events,
                        turn.thread_id,
                        session.settings.segment_id,
                        turn.turn_id,
                    )
                }),
        );
    }
    report
        .run(
            "agent.stop",
            "The harness and its sessions can be stopped",
            WITHIN,
            async {
                adapter
                    .stop()
                    .await
                    .map_err(|error| error.to_string().into())
            },
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::requests::{ModelRequestReport, UsageRequest};
    use orch_types::new_id;

    #[test]
    fn invented_zero_measurements_fail_conformance_but_nullable_counts_are_allowed() {
        let (thread, segment, turn) = (new_id(), new_id(), new_id());
        let event = |request| {
            AdapterEvent::ModelRequest(Box::new(ModelRequestReport {
                thread_id: thread,
                segment_id: segment,
                turn_id: turn,
                request,
            }))
        };
        let mut raw = UsageRequest {
            id: "zero-check".into(),
            ..Default::default()
        };
        raw.finish("done", None, None);
        assert!(requests(&[event(raw.clone())], thread, segment, turn).is_ok());
        raw.asked = Some("1970-01-01T00:00:00Z".parse().unwrap());
        assert!(requests(&[event(raw.clone())], thread, segment, turn).is_err());
        raw.asked = None;
        raw.output_streamed = Some(true);
        assert!(requests(&[event(raw.clone())], thread, segment, turn).is_err());
        raw.first_output = Some("2026-10-15T00:00:01Z".parse().unwrap());
        raw.tokens.output_tokens = Some(0);
        assert!(requests(&[event(raw.clone())], thread, segment, turn).is_err());
        raw.tokens.output_tokens = None;
        assert!(requests(&[event(raw)], thread, segment, turn).is_ok());
    }

    #[test]
    fn the_request_suite_accepts_unmeasured_reports_and_rejects_unfinished_or_misordered_ones() {
        let (thread, segment, turn) = (new_id(), new_id(), new_id());
        let mut request = UsageRequest {
            id: "one".into(),
            ..Default::default()
        };
        let event = |request| {
            AdapterEvent::ModelRequest(Box::new(ModelRequestReport {
                thread_id: thread,
                segment_id: segment,
                turn_id: turn,
                request,
            }))
        };
        assert!(requests(&[event(request.clone())], thread, segment, turn).is_err());
        request.finish("done", None, None);
        assert!(requests(&[event(request.clone())], thread, segment, turn).is_ok());
        request.asked = Some("2026-10-15T00:00:02Z".parse().unwrap());
        request.first_output = Some("2026-10-15T00:00:01Z".parse().unwrap());
        assert!(requests(&[event(request)], thread, segment, turn).is_err());
    }
}
