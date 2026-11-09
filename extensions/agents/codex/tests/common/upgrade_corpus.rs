//! Program-specific dispatch into the shared corpus and replayer; no protocol parsing lives here.
#![allow(
    dead_code,
    reason = "the notification and control binaries use separate seams"
)]

use orch_adapter_codex::process::automation_pins;
use orch_testkit::harness_corpus::{Corpus, Frame, Scenario, fixture_root};
use std::sync::OnceLock;

pub const VERSIONS: &[&str] = &["0.165.0", "0.167.0"];
pub const NOTIFICATIONS: &[&str] = &[
    "tool_command",
    "tool_read",
    "tool_search",
    "tool_file_change",
    "tool_web_search",
    "tool_web_fetch",
    "tool_web_find",
    "tool_mcp",
    "tool_subagent",
    "subagents",
    "tool_plan",
    "usage_limit",
    "rate_limit",
    "overloaded",
    "sign_in_expired",
    "sign_in_revoked",
    "context_overflow",
    "compaction_automatic",
    "compaction_manual",
    "compaction_marker",
    "compaction_idle",
    "model_reroute",
    "mcp_startup",
    "mcp_failure",
    "hooks",
    "tolerance_unknown_fields",
    "tolerance_optional_fields",
    "tolerance_unknown_event",
    "tolerance_unknown_method",
    "tolerance_unknown_enum",
    "tolerance_malformed",
    "transport_exit",
    "duplicate_turn_started",
];
const CONTROLS: &[&str] = &[
    "start",
    "resume",
    "fork",
    "session_close",
    "session_unload",
    "interrupt",
    "steer",
    "approval_command",
    "approval_file_change",
    "approval_permissions",
    "approval_mcp",
    "approval_unknown",
    "approval_unknown_method",
    "approval_checkpoint",
    "approval_prompt_user_rule",
    "approval_network",
    "user_input",
    "mcp_elicitation",
    "settings_program",
    "settings_server",
    "automatic_divergence",
    "background_end_unavailable",
    "approval_plan",
    "process_exit",
    "broken_stream",
    "background_wake",
    "background_end",
];

pub fn pin_case(pin: &str) -> String {
    format!("pin_{}", pin.replace('.', "_"))
}

pub fn controls() -> Vec<String> {
    CONTROLS
        .iter()
        .map(|name| (*name).to_owned())
        .chain(automation_pins().keys().map(|pin| pin_case(pin)))
        .collect()
}

pub fn build(version: &str) -> &'static Corpus {
    static OLDER: OnceLock<Corpus> = OnceLock::new();
    static CURRENT: OnceLock<Corpus> = OnceLock::new();
    let slot = match version {
        "0.165.0" => &OLDER,
        "0.167.0" => &CURRENT,
        _ => panic!("an undeclared corpus version: {version}"),
    };
    slot.get_or_init(|| {
        Corpus::load(&fixture_root().join("codex").join(version))
            .unwrap_or_else(|error| panic!("{version}: {error}"))
    })
}

pub fn expected(version: &str, name: &str) -> Scenario {
    build(version).manifest.scenarios[name].clone()
}

pub fn frames(version: &str, name: &str) -> Vec<Frame> {
    build(version).frames[name].clone()
}
