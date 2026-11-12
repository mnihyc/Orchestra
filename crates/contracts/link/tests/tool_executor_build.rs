//! Tool tickets retain the executor build they admitted without changing other tunnel kinds.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use orch_link_contract::tunnels::TicketClaims;
use serde_json::{Value, json};

fn claims() -> Value {
    json!({
        "jti":"0199c1a0-0000-7000-8000-00000000000a","kind":"exec","src":"api",
        "dst":"0199c1a0-0000-7000-8000-00000000000b","service":"exec/codex-exec-server",
        "tool_environment_id":"0199c1a0-0000-7000-8000-00000000000c",
        "tool_environment_generation":1,"exp":1791500000
    })
}

#[test]
fn an_exec_ticket_preserves_the_admitted_known_or_unknown_build() {
    for version in ["0.165.0", "unknown"] {
        let mut input = claims();
        input["tool_executor_version"] = json!(version);
        let read: TicketClaims = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(serde_json::to_value(read).unwrap(), input);
    }
}

#[test]
fn legacy_tool_and_non_executor_tickets_do_not_acquire_a_build() {
    for service in ["exec/codex-exec-server", "mcp/exec"] {
        let mut input = claims();
        input["service"] = json!(service);
        input["kind"] = json!(service.split('/').next().unwrap());
        let read: TicketClaims = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(serde_json::to_value(read).unwrap(), input);
    }
}
