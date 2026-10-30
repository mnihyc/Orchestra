//! Executor contract for tool execution inside an environment.
//!
//! An executor starts something inside an environment that speaks the protocol a harness expects, and hands
//! back a loopback address. Orch's own executor offers its tools as an MCP server in tool sets; Codex's is
//! the vendor's own program carrying the vendor's protocol; a package may bring another. They are peers: an
//! adapter names the executors its harness can work through ([`orch_agent_contract::descriptor`]), an
//! environment says what its exec node offers, in the order to try them, and a thread takes the first offer
//! its harness can work through.
//!
//! | Module | What it holds |
//! |--------|---------------|
//! | [`executor`] | The trait, and where a started executor is reached |
//! | [`descriptor`] | What an executor declares, and what it reports of itself in an environment |
//! | [`context`] | What an executor is given about the environment it starts in |

#![forbid(unsafe_code)]

pub mod context;
pub mod descriptor;
pub mod executor;
