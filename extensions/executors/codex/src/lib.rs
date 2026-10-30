//! Codex exec-server hosted inside a thread's environment.
//!
//! An environment's exec node starts [`server::CodexExecServer`] for a harness that runs in its provider's
//! runtime and reaches its tools through Codex's own exec server. The executor is the vendor's program,
//! `codex exec-server` of the build the environment's tool cache holds, listening on the loopback address on
//! a port the system gives and asking for a token. It carries the vendor's protocol, which this crate does
//! not speak: the node connects its tunnels to the address and the harness's own client speaks to the server.
//! What this crate adds is what the node needs to host it: finding the build, starting and ending the server
//! with an environment of its own making, reporting which build it is, and asking the program which sandbox
//! modes it can enforce in the place it runs.
//!
//! | Module | What it holds |
//! |--------|---------------|
//! | [`server`] | The executor: starting and ending the server, and the endpoint it hands back |
//! | [`descriptor`] | What the executor declares, the build it runs, and the variables a command carries |
//! | [`sandbox`] | The probe of the sandbox modes the place can enforce |

#![forbid(unsafe_code)]

pub mod descriptor;
pub mod sandbox;
pub mod server;
