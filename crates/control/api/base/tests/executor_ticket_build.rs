//! Stored native tool requests carry their admitted build through the actual ticket signer.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use std::sync::Arc;

use orch_api_core::tickets::{Tickets, request};
use orch_link_contract::tunnels::{Service, TicketSource, ToolSessionScope};
use orch_types::{Clock, Json, ManualClock, new_id};
use serde_json::json;

fn issued(service: Service, version: Option<&str>) -> Json {
    let clock = Arc::new(ManualClock::new(orch_testkit::clock::start()));
    let tickets = Tickets::generate(clock.clone());
    let (machine, environment) = (new_id(), new_id());
    let thread = new_id();
    let mut wanted = request(service.kind(), &service, new_id(), Some(thread));
    let scope = ToolSessionScope {
        thread_id: thread,
        segment_id: new_id(),
        opening_id: new_id(),
        nonce: new_id(),
        revision: 1,
        member: if matches!(service, Service::Mcp { .. }) {
            format!("mcp:{}", new_id())
        } else {
            "exec".into()
        },
    };
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
    let signed = tickets
        .for_session_request(&wanted, machine, &scope)
        .unwrap();
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
fn exec_without_a_build_and_mcp_keep_their_own_service_fields_inside_the_physical_scope() {
    let exec = issued(
        Service::Exec {
            executor: "codex-exec-server".into(),
        },
        None,
    );
    assert!(exec.get("tool_executor_version").is_none());
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
