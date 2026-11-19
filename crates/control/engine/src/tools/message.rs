//! One agent-facing message tool, routed by the intended recipient without changing either recipient's rights.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::tools::definitions::Refusal;
use crate::tools::{Answer, CallContext, ToolSettings};
use orch_agent_contract::events::OrchTool;
use orch_db::queries::threads::ThreadRow;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Chat,
    Thread,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    kind: Kind,
    path: String,
}

#[derive(Debug, Deserialize)]
struct Arguments {
    target: Target,
    text: Option<String>,
    files: Option<Vec<String>>,
    reply_to: Option<i64>,
}

/// The existing destination handlers receive the same message and resolve its path in their own way.
pub(super) struct Message {
    pub(super) path: String,
    pub(super) text: Option<String>,
    pub(super) files: Option<Vec<String>>,
    pub(super) reply_to: Option<i64>,
}

impl Engine {
    pub(crate) async fn send_message_call(
        &self,
        call: &CallContext,
        thread: &ThreadRow,
        settings: &ToolSettings,
    ) -> Result<Answer, EngineError> {
        let arguments: Arguments = match serde_json::from_value(call.arguments.clone()) {
            Ok(arguments) => arguments,
            Err(error) => return Ok(Refusal::invalid(error.to_string()).into()),
        };
        let legacy = match arguments.target.kind {
            Kind::Chat => "send_chat_message",
            Kind::Thread => {
                if arguments.reply_to.is_some() {
                    return Ok(Refusal::invalid("\"reply_to\" answers a chat message; it is only allowed when target.kind is \"chat\".").into());
                }
                "send_thread_message"
            }
        };
        // An old setting that enabled one destination does not acquire the other destination's authority.
        if !settings
            .enabled
            .iter()
            .any(|name| name == OrchTool::SendMessage.name() || name == legacy)
        {
            return Ok(Refusal::invalid(
                "send_message is not enabled for this target kind in orch_tools.enabled. Ask the place's managers to enable it."
            ).into());
        }
        let message = Message {
            path: arguments.target.path,
            text: arguments.text,
            files: arguments.files,
            reply_to: arguments.reply_to,
        };
        match arguments.target.kind {
            Kind::Chat => self.send_chat_message_call(call, thread, message).await,
            Kind::Thread => self.send_thread_message_call(call, thread, message).await,
        }
    }
}
