//! The Agent Client Protocol, as far as Orch speaks it: method names, the shapes the adapter reads and writes, small readers of the rest.
//!
//! The protocol is version 1 of the Agent Client Protocol (schema release 1.27). Only what the adapter needs is typed
//! here: what an agent says of itself in `initialize`, and the session's configuration options. Everything else
//! (updates, tool calls, requests) is read from the JSON as it arrives with the small readers below, so that a member
//! an agent adds, or leaves out where the schema allows it, never fails a conversation.

use orch_types::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// The protocol version the adapter speaks.
pub const PROTOCOL_VERSION: i64 = 1;

/// The methods an agent answers.
pub mod agent {
    /// Negotiates capabilities.
    pub const INITIALIZE: &str = "initialize";
    /// Starts a conversation.
    pub const SESSION_NEW: &str = "session/new";
    /// Loads a conversation and replays it as updates.
    pub const SESSION_LOAD: &str = "session/load";
    /// Resumes a conversation without replay.
    pub const SESSION_RESUME: &str = "session/resume";
    /// Ends a conversation and frees what it held.
    pub const SESSION_CLOSE: &str = "session/close";
    /// Lists the conversations the agent holds.
    pub const SESSION_LIST: &str = "session/list";
    /// Sends a prompt.
    pub const SESSION_PROMPT: &str = "session/prompt";
    /// Stops what the agent does for a prompt (a notification).
    pub const SESSION_CANCEL: &str = "session/cancel";
    /// Sets the mode of a session.
    pub const SESSION_SET_MODE: &str = "session/set_mode";
    /// Sets one configuration option of a session.
    pub const SESSION_SET_CONFIG_OPTION: &str = "session/set_config_option";
}

/// The methods a client answers, and the notification it receives.
pub mod client {
    /// What the agent did or learned (a notification).
    pub const SESSION_UPDATE: &str = "session/update";
    /// A request to be allowed something.
    pub const REQUEST_PERMISSION: &str = "session/request_permission";
    /// Reads a text file of the client's environment.
    pub const FS_READ: &str = "fs/read_text_file";
    /// Writes a text file of the client's environment.
    pub const FS_WRITE: &str = "fs/write_text_file";
    /// Starts a command.
    pub const TERMINAL_CREATE: &str = "terminal/create";
    /// Reads the output so far.
    pub const TERMINAL_OUTPUT: &str = "terminal/output";
    /// Waits for the command to end.
    pub const TERMINAL_WAIT: &str = "terminal/wait_for_exit";
    /// Ends the command, keeping the terminal.
    pub const TERMINAL_KILL: &str = "terminal/kill";
    /// Frees the terminal.
    pub const TERMINAL_RELEASE: &str = "terminal/release";
    /// Asks the person for structured input.
    pub const ELICITATION_CREATE: &str = "elicitation/create";
    /// Says that a URL elicitation ended (a notification).
    pub const ELICITATION_COMPLETE: &str = "elicitation/complete";
}

/// Orch's own messages, which an agent's companion plugin and the adapter exchange. Their names start with an
/// underscore, which the protocol reserves for extensions.
pub mod orch {
    /// The plugin says what it can do (a notification of the agent, or a request of the client).
    pub const HELLO: &str = "_orch/hello";
    /// An extra went off while the program ran (a notification of the agent).
    pub const EXTRA_STATE: &str = "_orch/extra_state";
    /// Live deltas of one model call (a notification of the agent).
    pub const STREAM: &str = "_orch/stream";
    /// Adds to the running turn (a request of the client).
    pub const INPUT_STEER: &str = "_orch/input/steer";
    /// What became of a message added to a turn (a notification of the agent).
    pub const INPUT_STATE: &str = "_orch/input/state";
    /// Asks a person questions (a request of the agent).
    pub const QUESTION_ASK: &str = "_orch/question/ask";
    /// A question asked earlier is not waited for any more (a notification of the agent).
    pub const QUESTION_CLOSED: &str = "_orch/question/closed";
    /// Switches plan mode (a request of the client).
    pub const PLAN_SET: &str = "_orch/plan/set";
    /// Plan mode changed (a notification of the agent).
    pub const PLAN_STATE: &str = "_orch/plan/state";
    /// The model submitted a plan for review (a request of the agent).
    pub const PLAN_REVIEW: &str = "_orch/plan/review";
    /// A call was refused because plan mode is on (a notification of the agent).
    pub const PLAN_REFUSED: &str = "_orch/plan/refused";
    /// Chooses whether changes ask for approval or run (a request of the client).
    pub const APPROVAL_SET: &str = "_orch/approval/set";
    /// One model call of the agent, with its tokens (a notification of the agent).
    pub const USAGE: &str = "_orch/usage";
    /// A phase of a compaction (a notification of the agent).
    pub const COMPACTION: &str = "_orch/compaction";
    /// Writes Orch's neutral history into a new session (a request of the client).
    pub const HISTORY_IMPORT: &str = "_orch/history/import";
    /// Copies a session at a turn (a request of the client).
    pub const SESSION_FORK: &str = "_orch/session/fork";
    /// What the plugin's own methods start with.
    pub const PREFIX: &str = "_orch/";
}

/// What the agent says of itself in its answer to `initialize`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Initialized {
    /// The version it speaks.
    pub protocol_version: i64,
    /// What it can do.
    pub agent_capabilities: AgentCapabilities,
    /// Who it is.
    pub agent_info: Option<Implementation>,
    /// How it signs in, where it does.
    pub auth_methods: Vec<Json>,
    /// Anything beyond the protocol, under the key `orch` for what Orch's plugin says.
    #[serde(rename = "_meta")]
    pub meta: Option<Json>,
}

/// An agent's or a client's name and version.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Implementation {
    /// The name for programs.
    pub name: String,
    /// The name for people, where it has one.
    pub title: Option<String>,
    /// The version.
    pub version: String,
}

/// What an agent can do.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentCapabilities {
    /// It loads a session and replays its history.
    pub load_session: bool,
    /// What a prompt may hold.
    pub prompt_capabilities: PromptCapabilities,
    /// Which MCP servers it takes.
    pub mcp_capabilities: McpCapabilities,
    /// Which session methods it has.
    pub session_capabilities: SessionCapabilities,
}

/// What a prompt may hold.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PromptCapabilities {
    /// Images.
    pub image: bool,
    /// Audio.
    pub audio: bool,
    /// Whole resources, embedded.
    pub embedded_context: bool,
}

/// Which MCP servers an agent takes, besides those it starts as programs, which every agent takes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct McpCapabilities {
    /// Servers reached by an address, with streamed answers.
    pub http: bool,
    /// Servers reached by an address, by the older event stream.
    pub sse: bool,
}

/// The session methods an agent has beyond `new`, `prompt` and `cancel`: each present as an object, even an empty one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SessionCapabilities {
    /// `session/list`.
    pub list: Option<Json>,
    /// `session/resume`.
    pub resume: Option<Json>,
    /// `session/close`.
    pub close: Option<Json>,
    /// `session/delete`.
    pub delete: Option<Json>,
}

/// One configuration option of a session: a choice, or a switch.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigOption {
    /// Its id, which `session/set_config_option` names.
    pub id: String,
    /// What it is called.
    pub name: String,
    /// What it is for: `model`, `mode`, `thought_level`, or another word.
    pub category: Option<String>,
    /// Its value now.
    pub current: Json,
    /// Its choices, each with the value that selects it and its name; empty for a switch.
    pub choices: Vec<Choice>,
    /// Whether it is a switch.
    pub boolean: bool,
}

/// One choice of a selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// What selects it.
    pub value: String,
    /// What it is called.
    pub name: String,
    /// The group it is listed in, where the agent groups them.
    pub group: Option<String>,
}

impl ConfigOption {
    /// Reads the options an agent lists (`configOptions`): selectors, flat or grouped, and switches.
    #[must_use]
    pub fn read_all(value: Option<&Json>) -> Vec<Self> {
        value
            .and_then(Json::as_array)
            .map(|options| options.iter().filter_map(Self::read).collect())
            .unwrap_or_default()
    }

    fn read(option: &Json) -> Option<Self> {
        let id = option.get("id")?.as_str()?.to_owned();
        let name = option
            .get("name")
            .and_then(Json::as_str)
            .unwrap_or(&id)
            .to_owned();
        let category = option
            .get("category")
            .and_then(Json::as_str)
            .map(str::to_owned);
        let boolean = option.get("type").and_then(Json::as_str) == Some("boolean");
        let current = option.get("currentValue").cloned().unwrap_or(Json::Null);
        let mut choices = Vec::new();
        for entry in option
            .get("options")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(group) = entry.get("group").and_then(Json::as_str) {
                for inner in entry
                    .get("options")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(choice) = Self::choice(inner, Some(group)) {
                        choices.push(choice);
                    }
                }
            } else if let Some(choice) = Self::choice(entry, None) {
                choices.push(choice);
            }
        }
        Some(Self {
            id,
            name,
            category,
            current,
            choices,
            boolean,
        })
    }

    fn choice(entry: &Json, group: Option<&str>) -> Option<Choice> {
        let value = entry.get("value")?.as_str()?.to_owned();
        let name = entry
            .get("name")
            .and_then(Json::as_str)
            .unwrap_or(&value)
            .to_owned();
        Some(Choice {
            value,
            name,
            group: group.map(str::to_owned),
        })
    }

    /// The choice that stands for `wanted`: the one whose value is it, whose name is it, or, for a value that is
    /// a JSON list of texts (a route written as `["provider","model"]`), whose last text is it.
    #[must_use]
    pub fn choose(&self, wanted: &str) -> Option<&Choice> {
        let last_of_list = |value: &str| {
            serde_json::from_str::<Vec<String>>(value)
                .ok()
                .and_then(|list| list.last().cloned())
        };
        self.choices
            .iter()
            .find(|choice| choice.value == wanted)
            .or_else(|| self.choices.iter().find(|choice| choice.name == wanted))
            .or_else(|| {
                self.choices
                    .iter()
                    .find(|choice| last_of_list(&choice.value).as_deref() == Some(wanted))
            })
    }

    /// The value now, as text, where it is text.
    #[must_use]
    pub fn current_text(&self) -> Option<&str> {
        self.current.as_str()
    }
}

/// The session modes an agent has, and the one it is in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Modes {
    /// The id of the mode it is in.
    pub current: String,
    /// The modes: id and name.
    pub available: Vec<(String, String)>,
}

impl Modes {
    /// Reads `modes` of an answer to `session/new`, `session/load` or `session/resume`.
    #[must_use]
    pub fn read(value: Option<&Json>) -> Option<Self> {
        let value = value?;
        let current = value.get("currentModeId")?.as_str()?.to_owned();
        let available = value
            .get("availableModes")
            .and_then(Json::as_array)?
            .iter()
            .filter_map(|mode| {
                let id = mode.get("id")?.as_str()?.to_owned();
                let name = mode
                    .get("name")
                    .and_then(Json::as_str)
                    .unwrap_or(&id)
                    .to_owned();
                Some((id, name))
            })
            .collect();
        Some(Self { current, available })
    }

    /// Whether the agent has a mode of that id.
    #[must_use]
    pub fn has(&self, id: &str) -> bool {
        self.available.iter().any(|(mode, _)| mode == id)
    }
}

/// The text of a content block, where it is text; a link as Markdown; an image or audio as a bracketed word.
#[must_use]
pub fn content_text(block: &Json) -> Option<String> {
    match block.get("type").and_then(Json::as_str)? {
        "text" => block.get("text").and_then(Json::as_str).map(str::to_owned),
        "resource_link" => {
            let uri = block.get("uri").and_then(Json::as_str)?;
            let name = block.get("name").and_then(Json::as_str).unwrap_or(uri);
            Some(format!("[{name}]({uri})"))
        }
        "resource" => block
            .pointer("/resource/text")
            .and_then(Json::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

/// A text block of a prompt.
#[must_use]
pub fn text_block(text: &str) -> Json {
    json!({ "type": "text", "text": text })
}

/// An image block of a prompt.
#[must_use]
pub fn image_block(base64: &str, media_type: &str) -> Json {
    json!({ "type": "image", "data": base64, "mimeType": media_type })
}

/// The kinds of stop of a prompt the protocol names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The turn ended well.
    EndTurn,
    /// The agent hit the most tokens it may write.
    MaxTokens,
    /// The agent hit the most model requests between two prompts.
    MaxTurnRequests,
    /// The agent refused to go on.
    Refusal,
    /// The client cancelled.
    Cancelled,
    /// A reason this adapter does not know; the turn is taken as failed.
    Other,
}

impl StopReason {
    /// Reads the reason of an answer to `session/prompt`.
    #[must_use]
    pub fn read(answer: &Json) -> Self {
        match answer.get("stopReason").and_then(Json::as_str) {
            Some("end_turn") => Self::EndTurn,
            Some("max_tokens") => Self::MaxTokens,
            Some("max_turn_requests") => Self::MaxTurnRequests,
            Some("refusal") => Self::Refusal,
            Some("cancelled") => Self::Cancelled,
            _ => Self::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_an_agent_says_of_itself_reads_with_every_absent_member_as_not_claimed() {
        let read: Initialized = serde_json::from_value(json!({
            "protocolVersion": 1,
            "agentInfo": { "name": "deepseek-harness-acp", "version": "0.0.1" },
            "agentCapabilities": {
                "mcpCapabilities": { "http": true },
                "promptCapabilities": { "image": false, "audio": false, "embeddedContext": false },
                "sessionCapabilities": { "close": {}, "list": {}, "resume": {} }
            },
            "authMethods": []
        }))
        .unwrap();
        let caps = read.agent_capabilities;
        assert!(!caps.load_session && !caps.prompt_capabilities.image);
        assert!(caps.mcp_capabilities.http && !caps.mcp_capabilities.sse);
        assert!(
            caps.session_capabilities.resume.is_some()
                && caps.session_capabilities.delete.is_none()
        );
        assert_eq!(read.agent_info.unwrap().version, "0.0.1");
        let bare: Initialized = serde_json::from_value(json!({ "protocolVersion": 1 })).unwrap();
        assert_eq!(bare.agent_capabilities, AgentCapabilities::default());
    }

    #[test]
    fn options_are_read_flat_or_in_groups_and_a_choice_is_found_by_value_name_or_the_last_of_a_route()
     {
        let options = ConfigOption::read_all(Some(&json!([
            { "id": "model", "name": "Model", "category": "model", "type": "select",
              "currentValue": "[\"orch\",\"deepseek-v4-flash\"]",
              "options": [ { "group": "orch", "name": "Orch", "options": [
                  { "value": "[\"orch\",\"deepseek-v4-flash\"]", "name": "DeepSeek V4 Flash" },
                  { "value": "[\"orch\",\"deepseek-v4-pro\"]", "name": "DeepSeek V4 Pro" } ] } ] },
            { "id": "reasoning_effort", "name": "Effort", "category": "thought_level", "type": "select",
              "currentValue": "", "options": [ { "value": "", "name": "Provider default" }, { "value": "high", "name": "High" } ] },
            { "id": "fast", "name": "Fast", "type": "boolean", "currentValue": false }
        ])));
        assert_eq!(options.len(), 3);
        assert_eq!(options[0].choices.len(), 2);
        assert_eq!(options[0].choices[0].group.as_deref(), Some("orch"));
        let by_route = options[0].choose("deepseek-v4-pro").unwrap();
        assert_eq!(by_route.value, "[\"orch\",\"deepseek-v4-pro\"]");
        assert_eq!(
            options[0].choose("DeepSeek V4 Flash").unwrap().name,
            "DeepSeek V4 Flash"
        );
        assert!(options[0].choose("missing").is_none());
        assert_eq!(options[1].choose("high").unwrap().name, "High");
        assert!(options[2].boolean && options[2].choices.is_empty());
    }

    #[test]
    fn content_is_read_as_text_where_it_is_text_and_a_link_as_markdown() {
        assert_eq!(content_text(&text_block("hi")).as_deref(), Some("hi"));
        assert_eq!(
            content_text(&json!({ "type": "resource_link", "uri": "file:///a", "name": "a" }))
                .as_deref(),
            Some("[a](file:///a)")
        );
        assert_eq!(
            content_text(&json!({ "type": "image", "data": "", "mimeType": "image/png" })),
            None
        );
    }

    #[test]
    fn a_stop_reason_the_adapter_does_not_know_is_not_taken_for_success() {
        assert_eq!(
            StopReason::read(&json!({ "stopReason": "cancelled" })),
            StopReason::Cancelled
        );
        assert_eq!(
            StopReason::read(&json!({ "stopReason": "something_new" })),
            StopReason::Other
        );
        assert_eq!(StopReason::read(&json!({})), StopReason::Other);
    }
}
