//! The suite every agent adapter passes, run against the generic adapter and an agent that speaks the protocol from a pipe.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::sync::Arc;

use common::{Agent, Starter, agent_start, program, spec_in, turn_for};
use orch_adapter_acp::AcpAdapter;
use orch_agent_contract::session::SessionMode;
use orch_conformance::agent::{self, Fixture};
use orch_conformance::report::Report;
use orch_types::{SystemClock, new_id};
use serde_json::json;

fn fixture(home: &std::path::Path, work: &std::path::Path) -> Fixture {
    let (provider, profile) = (new_id(), new_id());
    let sessions = vec![
        spec_in(work, profile, provider, SessionMode::New),
        spec_in(work, profile, provider, SessionMode::New),
    ];
    let turns = sessions
        .iter()
        .map(|spec| turn_for(spec, "Say what you were told."))
        .collect();
    Fixture {
        start: agent_start(provider, profile, home),
        sessions,
        turns,
    }
}

#[tokio::test]
async fn the_generic_adapter_passes_the_suite_of_every_agent_adapter_on_an_agent_it_has_no_code_for()
 {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let mut adapter = AcpAdapter::with_launcher(
        program(json!({})),
        Arc::new(Starter {
            agent: Agent::new(),
        }),
        Arc::new(SystemClock),
    );
    let mut report = Report::new("agent \"demo\" through the Agent Client Protocol");
    agent::run(
        &mut adapter,
        Some(&fixture(home.path(), work.path())),
        &mut report,
    )
    .await;
    let failures: Vec<String> = report
        .failures()
        .map(|case| format!("{}: {:?}", case.rule, case.outcome))
        .collect();
    assert!(failures.is_empty(), "{failures:#?}");
    let (passed, _, _) = report.counts();
    assert!(
        passed >= 5,
        "start, descriptor, probe, sessions, requests and stop ran: {passed}"
    );
}
