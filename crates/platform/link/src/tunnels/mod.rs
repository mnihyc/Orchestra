//! Tunnels components.
//!
//! A tunnel is an ordered stream of bytes between two machines, or between
//! the system and a machine, that a gateway relays and that survives the
//! loss of a session. [`channel`] is the tunnel as a program sees it and the
//! machine's tunnels as a whole; [`flow`] decides when credit is given back;
//! [`replay`] keeps what has been sent and not yet acknowledged; [`relay`] is
//! what a gateway does with the channels of the sessions it holds; and
//! [`terminal`] is the system's end of a tunnel that ends at a browser.

pub mod channel;
pub mod flow;
pub mod relay;
pub mod replay;
pub mod terminal;
