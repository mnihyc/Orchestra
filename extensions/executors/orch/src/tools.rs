//! The tool sets a harness works through when it has no tools of its own, or has them switched off: which one a session gets from what the environment offers, and what each lists; none for a session that is to act through the environment's own servers alone.
//!
//! There are three sets. `claude-code` and `plain` list tools; `custom` lists none, because a session that
//! has it acts through the MCP servers of the environment alone and is not given an `exec` server at all.
//! Which set a thread gets is worked out from the environment's offers and what its harness can work with
//! (the arrangement choice of the agent contract); this module holds the names, what each set lists, which
//! sets this executor can offer now, and the vocabulary every tool answers in: [`ToolOutput`] and
//! [`ToolFault`].

use std::sync::OnceLock;

use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::commands::Commands;
use crate::sandbox::{Access, Refusal};
use crate::scope::SessionScope;

/// One of the three sets of tools `orch-exec` can offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolSet {
    /// Tools with the names and inputs of Claude Code's own shell and file tools.
    #[serde(rename = "claude-code")]
    ClaudeCode,
    /// Eight plain tools for any harness and any model.
    #[serde(rename = "plain")]
    Plain,
    /// No tools of its own: the agent acts through the environment's MCP servers.
    #[serde(rename = "custom")]
    Custom,
}

impl ToolSet {
    /// Every set, in the order the descriptor lists them.
    pub const ALL: [Self; 3] = [Self::ClaudeCode, Self::Plain, Self::Custom];

    /// The name an environment's offers, an adapter's entries and a ticket use.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Plain => "plain",
            Self::Custom => "custom",
        }
    }

    /// The set called `name`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|set| set.name() == name)
    }

    /// One sentence for the screens.
    #[must_use]
    pub const fn summary(self) -> &'static str {
        match self {
            Self::ClaudeCode => {
                "Tools with the names and inputs of Claude Code's own shell and file tools."
            }
            Self::Plain => "Eight plain tools for any harness and any model.",
            Self::Custom => {
                "No tools of its own: the agent acts through the environment's MCP servers."
            }
        }
    }

    /// The sets this executor can offer now: `custom` only while at least one MCP server is enabled for
    /// the environment, since a session that has it would have nothing to act with otherwise.
    #[must_use]
    pub fn offered_now(mcp_servers_enabled: bool) -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|set| *set != Self::Custom || mcp_servers_enabled)
            .collect()
    }

    /// The set an entry of an environment's offers comes to: the name of a set it can offer now, or the
    /// executor's own name, which stands for the first set it offers.
    #[must_use]
    pub fn resolve(offer: &str, offered: &[Self]) -> Option<Self> {
        if offer == crate::descriptor::KIND {
            return offered.first().copied();
        }
        Self::parse(offer).filter(|set| offered.contains(set))
    }

    /// What the server lists for a session of this set. `custom` lists nothing.
    #[must_use]
    pub fn tools(self) -> &'static [Tool] {
        static CLAUDE_CODE: OnceLock<Vec<Tool>> = OnceLock::new();
        static PLAIN: OnceLock<Vec<Tool>> = OnceLock::new();
        match self {
            Self::ClaudeCode => CLAUDE_CODE.get_or_init(crate::claude_code::definitions),
            Self::Plain => PLAIN.get_or_init(crate::plain::definitions),
            Self::Custom => &[],
        }
    }

    /// The tool called `name` in this set.
    #[must_use]
    pub fn tool(self, name: &str) -> Option<&'static Tool> {
        self.tools().iter().find(|tool| tool.name == name)
    }
}

/// What a tool answers when it succeeds.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolOutput {
    /// A result that is an object: sent as structured content and, as text, the same JSON.
    Json(serde_json::Value),
    /// Text for the model to read.
    Text(String),
    /// An image, as the file holds it: base64 text and the media type.
    Image {
        /// The file's bytes, base64.
        data: String,
        /// The media type, such as `image/png`.
        media_type: String,
    },
}

/// Why a tool did not do what it was asked. It reaches the model as a result marked as an error, with a
/// code and the sentence below it, so that the model can correct the call.
#[derive(Debug, thiserror::Error)]
pub enum ToolFault {
    /// The input is not what the tool takes.
    #[error("{0}")]
    InvalidInput(String),
    /// The sandbox mode, or a read-only mount, does not allow it.
    #[error(transparent)]
    Refused(#[from] Refusal),
    /// Something asked for is not there.
    #[error("{0}")]
    NotFound(String),
    /// A directory where a file was meant.
    #[error("{0}")]
    IsDirectory(String),
    /// A file that is not text.
    #[error("{0}")]
    NotText(String),
    /// More than the tool takes.
    #[error("{0}")]
    TooLarge(String),
    /// The text to replace is not in the file.
    #[error("{0}")]
    NoMatch(String),
    /// The text to replace is in the file more than once.
    #[error("{0}")]
    Ambiguous(String),
    /// No background command of this thread has that id.
    #[error("{0}")]
    NoSuchProcess(String),
    /// The thread already has as many commands running in the background as it may.
    #[error("{0}")]
    TooManyProcesses(String),
    /// The command could not be started.
    #[error("{0}")]
    Unavailable(String),
    /// The client went away, or the executor is stopping, before the tool was done.
    #[error("the call was cancelled")]
    Cancelled,
    /// The operating system refused.
    #[error("{what}: {source}")]
    Io {
        /// What was being done, with the path.
        what: String,
        /// What the system said.
        #[source]
        source: std::io::Error,
    },
}

impl ToolFault {
    /// The code the result carries.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "invalid_input",
            Self::Refused(refusal) => refusal.code(),
            Self::NotFound(_) => "not_found",
            Self::IsDirectory(_) => "is_directory",
            Self::NotText(_) => "not_text",
            Self::TooLarge(_) => "too_large",
            Self::NoMatch(_) => "no_match",
            Self::Ambiguous(_) => "ambiguous",
            Self::NoSuchProcess(_) => "no_such_process",
            Self::TooManyProcesses(_) => "too_many_processes",
            Self::Unavailable(_) => "unavailable",
            Self::Cancelled => "cancelled",
            Self::Io { .. } => "io_error",
        }
    }

    /// An operating-system error on `path`, with what was being done.
    #[must_use]
    pub fn io(what: &str, path: &std::path::Path, source: std::io::Error) -> Self {
        if source.kind() == std::io::ErrorKind::NotFound {
            return Self::NotFound(format!("{what}: {} does not exist", path.display()));
        }
        Self::Io {
            what: format!("{what} {}", path.display()),
            source,
        }
    }
}

/// Annotations for a tool that only reads.
pub(crate) fn reads_only(title: &str) -> ToolAnnotations {
    ToolAnnotations::with_title(title)
        .read_only(true)
        .open_world(false)
}

/// Annotations for a tool that changes files or runs commands. A command can do anything the environment
/// allows, so it is destructive and its reach is open.
pub(crate) fn changes_things(title: &str, destructive: bool, idempotent: bool) -> ToolAnnotations {
    ToolAnnotations::with_title(title)
        .read_only(false)
        .destructive(destructive)
        .idempotent(idempotent)
        .open_world(false)
}

/// The input schema of a tool whose input is the type `T`.
///
/// A type that cannot be a tool's input is a fault of this program and is found by the tests that list
/// every set, so it is not carried as a typed error here.
pub(crate) fn schema<T: schemars::JsonSchema + std::any::Any>() -> std::sync::Arc<JsonObject> {
    rmcp::handler::server::tool::schema_for_input::<T>().unwrap_or_default()
}

/// What one tool call is given.
#[derive(Debug)]
pub struct Call<'a> {
    /// The session the call belongs to.
    pub scope: &'a SessionScope,
    /// What the session may reach.
    pub access: &'a Access,
    /// The commands of the executor.
    pub commands: &'a Commands,
    /// The id of the tool call: the value of `ORCH_ITEM_ID` in the commands it starts.
    pub call_id: &'a str,
    /// Fires when the client has gone away.
    pub cancel: &'a CancellationToken,
}

/// The arguments of a call as the tool's input type, or why they are not.
pub(crate) fn arguments<T: serde::de::DeserializeOwned>(given: JsonObject) -> Result<T, ToolFault> {
    serde_json::from_value(serde_json::Value::Object(given))
        .map_err(|error| ToolFault::InvalidInput(error.to_string()))
}

/// Runs `work` on a thread that may block, and answers what it answered.
pub(crate) async fn blocking<T, F>(work: F) -> Result<T, ToolFault>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ToolFault> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| ToolFault::Cancelled)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_is_found_by_the_name_the_environments_offers_use() {
        for set in ToolSet::ALL {
            assert_eq!(ToolSet::parse(set.name()), Some(set));
        }
        assert_eq!(ToolSet::parse("codex-exec-server"), None);
    }

    #[test]
    fn an_offer_comes_to_a_set_only_when_the_executor_can_offer_it() {
        let without_servers = ToolSet::offered_now(false);
        assert_eq!(
            ToolSet::resolve("plain", &without_servers),
            Some(ToolSet::Plain)
        );
        assert_eq!(ToolSet::resolve("custom", &without_servers), None);
        assert_eq!(
            ToolSet::resolve("custom", &ToolSet::offered_now(true)),
            Some(ToolSet::Custom)
        );
        assert_eq!(
            ToolSet::resolve("orch-exec", &without_servers),
            Some(ToolSet::ClaudeCode),
            "the executor's own name stands for the first set it offers"
        );
    }

    #[test]
    fn the_plain_set_lists_eight_tools_and_the_claude_code_set_seven_and_custom_none() {
        let plain: Vec<&str> = ToolSet::Plain
            .tools()
            .iter()
            .map(|tool| &*tool.name)
            .collect();
        assert_eq!(
            plain,
            [
                "shell",
                "shell_output",
                "shell_stop",
                "read",
                "write",
                "edit",
                "list",
                "search"
            ]
        );
        let claude: Vec<&str> = ToolSet::ClaudeCode
            .tools()
            .iter()
            .map(|tool| &*tool.name)
            .collect();
        assert_eq!(
            claude,
            ["Bash", "Read", "Write", "Edit", "Glob", "Grep", "TaskStop"]
        );
        assert!(ToolSet::Custom.tools().is_empty());
    }

    #[test]
    fn every_tool_takes_an_object_and_refuses_members_it_does_not_list() {
        for set in [ToolSet::Plain, ToolSet::ClaudeCode] {
            for tool in set.tools() {
                assert_eq!(
                    tool.input_schema.get("type"),
                    Some(&serde_json::json!("object")),
                    "{} takes an object",
                    tool.name
                );
                assert_eq!(
                    tool.input_schema.get("additionalProperties"),
                    Some(&serde_json::json!(false)),
                    "{} refuses an unknown member",
                    tool.name
                );
                assert!(
                    tool.description.is_some(),
                    "{} has a description",
                    tool.name
                );
            }
        }
    }
}
