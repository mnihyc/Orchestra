//! Orch's own executor: plain exec tools served through MCP, one of the executors an environment offers.
//!
//! An environment's exec node starts [`server::OrchExec`] for a harness that runs in its provider's runtime
//! and has no tools of its own, or has them switched off. The executor listens on the loopback address, on a
//! port the system gives, and serves the tools of one of three sets, as an MCP server called `exec`: `plain`
//! (eight tools), `claude-code` (seven tools with the names and inputs of Claude Code's own shell and file
//! tools, carried out here) and `custom` (none: the session acts through the environment's MCP servers alone).
//! Which set a session has is the one its environment's offers come to for its harness; this crate carries it
//! out and chooses nothing.
//!
//! Every file operation and every command is held to the thread's directory and the attached mounts by the
//! agent's sandbox mode, and every command carries `ORCH_THREAD_ID` and `ORCH_ITEM_ID`, so that the
//! environment can say which thread a process belongs to.
//!
//! | Module | What it holds |
//! |--------|---------------|
//! | [`server`] | The executor, its endpoint, and the MCP service behind a token |
//! | [`scope`] | The session a request belongs to, as the connector states it in a header |
//! | [`descriptor`] | What the executor declares about itself |
//! | [`tools`] | The three tool sets, and how a tool answers or fails |
//! | [`plain`] | The eight plain tools |
//! | [`claude_code`] | The tools with the names and inputs of Claude Code's own |
//! | [`commands`] | Running commands, in the foreground and the background |
//! | [`output`] | Bounded views of a command's output, and the file that keeps it |
//! | [`files`] | Reading, writing, editing and listing |
//! | [`sandbox`] | Which paths a session may reach |
//! | [`search`] | Searching file contents and names |

#![forbid(unsafe_code)]

pub mod claude_code;
pub mod commands;
pub mod descriptor;
pub mod files;
pub mod output;
pub mod plain;
pub mod sandbox;
pub mod scope;
pub mod search;
pub mod server;
pub mod tools;
