//! Stored native tool requests carry their admitted build through the actual ticket signer.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use std::sync::Arc;

use orch_api_core::tickets::{Tickets, request};
use orch_link_contract::tunnels::{Service, TicketSource};
use orch_types::{Clock, Json, ManualClock, new_id};
use serde_json::json;

fn issued(service: Service, version: Option<&str>) -> Json {
    let clock = Arc::new(ManualClock::new(orch_testkit::clock::start()));
    let tickets = Tickets::generate(clock.clone());
    let (machine, environment) = (new_id(), new_id());
    let mut wanted = request(service.kind(), &service, new_id(), Some(new_id()));
    wanted
        .more
        .insert("environment_id".into(), json!(environment));
    wanted
        .more
        .insert("environment_generation".into(), json!(3));
    if let Some(version) = version {
        wanted
            .more
            .insert("executor_version".into(), json!(version));
    }
    let signed = tickets.for_request(&wanted, machine).unwrap();
    let claims = tickets
        .verifier()
        .verify(&signed, TicketSource::Machine(machine), clock.time())
        .unwrap();
    let projected = serde_json::to_value(claims).unwrap();
    assert_eq!(projected["tool_environment_id"], json!(environment));
    assert_eq!(projected["tool_environment_generation"], 3);
    projected
}

#[test]
fn an_exec_request_signs_its_admitted_build_including_explicit_unknown() {
    for version in ["0.165.0", "unknown"] {
        let claims = issued(
            Service::Exec {
                executor: "codex-exec-server".into(),
            },
            Some(version),
        );
        assert_eq!(claims["tool_executor_version"], version);
    }
}

#[test]
fn retained_exec_and_mcp_requests_keep_their_existing_ticket_shape() {
    let legacy = issued(
        Service::Exec {
            executor: "codex-exec-server".into(),
        },
        None,
    );
    assert!(legacy.get("tool_executor_version").is_none());
    for version in [None, Some("unused-on-mcp")] {
        let mcp = issued(
            Service::Mcp {
                server: "exec".into(),
            },
            version,
        );
        assert!(mcp.get("tool_executor_version").is_none());
    }
}
