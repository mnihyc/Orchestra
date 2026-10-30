//! Claude Code adapter; Claude Code-specific vocabulary stays here.
//!
//! [`adapter::ClaudeCodeAdapter`] drives the vendor's command-line program in its non-interactive mode, one
//! process for each conversation, through JSON lines on the process's standard streams: user messages in,
//! messages and requests out, and the control requests that protocol has for permissions, interrupts and
//! settings. Everything that leaves this package is neutral: the harness's tools become steps of a kind
//! (a command, a change of files, a read, a search, a fetch, a sub-agent), each kept whole beside the
//! understood form, and its permission prompts become approvals.
//!
//! | Module | What it holds |
//! |--------|---------------|
//! | [`adapter`] | The adapter itself: start, stop, sessions, turns, sign-in, discovery |
//! | [`conversation`] | One conversation: its process, its turn and the events it yields |
//! | [`config`] | The names and limits of the adapter, and the options of a profile |
//! | [`descriptor`] | What the adapter declares of the harness |
//! | [`process`] | Starting a process of the harness and talking to it |
//! | [`stream`] | The harness's JSON lines, read tolerantly and written exactly |
//! | [`sessions`] | The flags, settings and environment of one session's process |
//! | [`events`] | The harness's messages as neutral events |
//! | [`failure`] | What the harness says when a turn fails, and what Orch makes of it |
//! | [`approvals`] | Permission requests as approvals; approval and sandbox modes |
//! | [`provider`] | Signing in, the facts of a sign-in, credentials |
//! | [`tools`] | The harness's tools as steps of a kind; which tools a session has |
//! | [`discovery`] | Sessions and MCP servers the harness already has |
//! | [`history`] | Transcripts: export, import, resync |
//! | [`parameters`] | The adapter's own parameters and how model and effort reach the harness |

#![forbid(unsafe_code)]

pub mod adapter;
pub mod approvals;
pub mod config;
pub mod context_usage;
pub mod conversation;
pub mod descriptor;
pub mod discovery;
pub mod events;
pub mod failure;
pub mod history;
pub mod instruction_sources;
mod native_history;
pub mod parameters;
pub mod performance;
pub mod process;
pub mod provider;
pub mod review;
pub mod sessions;
pub mod stream;
pub mod tools;

pub use adapter::ClaudeCodeAdapter;
