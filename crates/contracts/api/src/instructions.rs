//! What a thread's agent reads as its instructions, source by source: Orch's introduction, the guide to its tools,
//! each place's standing instructions, and the files its harness reads (reported by the harness, or found by its
//! documented loading rule), as the last session was opened with them; and the standing instructions as they stand
//! now, apart, so that a change that waits for the next turn is not shown as sent.
//!
//! The list is the reader's own: a file's path is shown only to whoever may view the files of the environment it lies
//! in, a file of a person's home has only its label, and a place's name only to whoever may see the place. Reading
//! it makes no request of a model and changes nothing.

use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::environment::EnvironmentRef;
use crate::nodes::NodeRef;
use crate::routes::Access::Permission;

/// What kind of source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstructionSourceKind {
    /// Orch's introduction (the preamble).
    Preamble,
    /// One place's standing instructions.
    Standing,
    /// The guide to Orch's tools, where it goes with the instructions.
    Guide,
    /// A file the harness reads: a directory's, an override, a person's own, a rule, an import.
    File,
    /// The index of the agent's notes.
    Memory,
}

/// How it is known that the agent reads a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstructionEvidence {
    /// Orch composed it and gave it.
    Orch,
    /// The harness said so when the session was opened.
    Reported,
    /// The harness's documented loading rule says so; the harness said nothing.
    DocumentedRule,
}

/// One source of what the agent reads, in its place in the order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstructionSource {
    /// Its place in the order, from 0.
    pub order: u32,
    /// What kind of source.
    pub kind: InstructionSourceKind,
    /// What it is, in a few words that name no home.
    pub label: String,
    /// The place whose standing instructions these are.
    pub scope: Option<NodeRef>,
    /// The environment a file lies in, for whoever may see it.
    pub source_environment: Option<EnvironmentRef>,
    /// Where the file is, for whoever may view the files of its environment; never a path in a person's home.
    pub path: Option<String>,
    /// The same permitted file from its environment's workspace root, suitable for the file viewer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
    /// How it is known that the agent reads it.
    pub evidence: InstructionEvidence,
    /// How many bytes it holds; `null` where that is not known.
    pub bytes: Option<u64>,
    /// How many of them the agent takes in; `null` where that is not known.
    pub loaded_bytes: Option<u64>,
    /// How many are left out by a bound; `null` where that is not known.
    pub cut_bytes: Option<u64>,
    /// How many lines it holds; `null` where that is not known.
    pub lines: Option<u64>,
    /// How many of them the agent takes in; `null` where that is not known.
    pub loaded_lines: Option<u64>,
    /// Why a value is not known, or how it was derived.
    pub reason: Option<String>,
    /// The native reason for a later load, where the harness reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_reason: Option<String>,
    /// The importing file, shown only with the same file reach as this source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_path: Option<String>,
    /// A separate documented line limit (for example the notes index).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_limit: Option<u64>,
    /// A separate documented byte limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_limit: Option<u64>,
    /// Whether the reader may open it: a file of an environment whose files they may view, or what Orch gave
    /// (read in what the agent was given).
    pub readable: bool,
    /// Whether the reader may change it here: a file of the workspace whose files they may change.
    pub editable: bool,
}

/// The standing instructions of the thread as they stand now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StandingNow {
    /// How many characters they come to, with their headings.
    pub characters: u64,
    /// The most they may come to.
    pub limit: u64,
    /// Whether they differ from what the last session was opened with: the change applies with the next turn.
    pub pending_next_turn: bool,
}

/// What a thread's agent reads as its instructions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadInstructions {
    /// The segment whose session the sources are of; `null` before a session was opened.
    pub segment_id: Option<Id>,
    /// When that session was last opened with them; `null` where that is not known.
    pub observed_at: Option<Time>,
    /// The sources, in the order the agent reads them.
    pub sources: Vec<InstructionSource>,
    /// The standing instructions as they stand now.
    pub standing: StandingNow,
    /// What was last told of where the agent is, as a revision; `null` before anything was told.
    pub context_revision: Option<String>,
}

crate::endpoints! {
    /// Reads what the thread's agent reads as its instructions, source by source, as the reader may see them.
    ThreadInstructionsGet: GET "/threads/{id}/instructions", Permission("thread.view"), none, none, json ThreadInstructions, 200;
}
