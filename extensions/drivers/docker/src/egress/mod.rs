//! The host's proxy for restricted units: what it admits, the files the node and the proxy share, the HTTP proxy, the filtered resolver and the program that runs them.
//!
//! A restricted unit (a template's `network.egress` is `restricted`, or its `network.mode` is `none`) is put on an
//! internal network of its own whose bridge has no address on the host, with one other member: a proxy the node
//! runs for it in a container of its own. The proxy is also on an ordinary network and is the unit's only way out.
//! It admits Orch's trusted transport (the gateway, by its exact name and port, with the link's own pinned TLS
//! carried through untouched) and the other host names the unit's place allows, decided before any lookup or
//! socket; everything else is refused with the host and a reason, and nothing of a request is kept.
//!
//! | Module | What it holds |
//! |--------|---------------|
//! | [`admission`] | Canonical host names, what the proxy admits and which addresses no other name may reach |
//! | [`files`] | The orders the node writes, the state the proxy answers, and the node's heartbeat |
//! | [`proxy`] | The HTTP proxy: `CONNECT` and plain requests, and the flows it may close |
//! | [`resolver`] | The filtered resolver the unit's own name lookups go to |
//! | [`program`] | The proxy program of one unit: its listeners, its orders and its state |

pub mod admission;
pub mod files;
pub mod program;
pub mod proxy;
pub mod resolver;
