//! Browser directives call the browser tool and say its result as an agent message.

use super::{OrchCall, ScriptError, fault};
use orch_agent_contract::events::OrchTool;
use serde_json::json;

/// Parses the command and its profile, endpoint or identifier without asking for a calling thread.
pub fn parse(line: usize, rest: &str) -> Result<OrchCall, ScriptError> {
    let (command, value) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(command, value)| (command, value.trim()));
    let mut arguments = json!({"command":command});
    let member = match command {
        "list" => None,
        "open" => Some("profile"),
        "announce" => Some("endpoint"),
        "close" => Some("browser"),
        _ => {
            return Err(fault(
                line,
                "browser is: browser list|open|announce|close [profile|endpoint|id]",
            ));
        }
    };
    if let Some(member) = member {
        if !value.is_empty() {
            arguments[member] = json!(value);
        }
    } else if !value.is_empty() {
        return Err(fault(line, "browser list takes no more arguments"));
    }
    Ok(OrchCall {
        tool: OrchTool::UseBrowser,
        arguments,
    })
}
