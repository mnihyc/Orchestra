//! Display directives call the display tool and say its safe result as an agent message.

use super::{OrchCall, ScriptError, fault};
use orch_agent_contract::events::OrchTool;
use serde_json::json;

/// Parses a display command with its geometry, endpoint or identifier.
pub fn parse(line: usize, rest: &str) -> Result<OrchCall, ScriptError> {
    let (command, value) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(command, value)| (command, value.trim()));
    let mut arguments = json!({"command":command});
    let member = match command {
        "list" => None,
        "open" => Some("geometry"),
        "announce" => Some("endpoint"),
        "close" => Some("display"),
        _ => {
            return Err(fault(
                line,
                "display is: display list|open|announce|close [geometry|endpoint|id]",
            ));
        }
    };
    if let Some(member) = member {
        if !value.is_empty() {
            arguments[member] = json!(value);
        }
    } else if !value.is_empty() {
        return Err(fault(line, "display list takes no more arguments"));
    }
    Ok(OrchCall {
        tool: OrchTool::UseDisplay,
        arguments,
    })
}
