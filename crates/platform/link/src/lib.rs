//! Shared reliable link mechanisms; persistence of product state belongs to the API.
//!
//! A machine keeps one session open to a gateway, and everything between the
//! machine and the system travels in it: events that must arrive once and in
//! order, commands that must run once, and tunnels of bytes that must arrive
//! whole. The session breaks whenever a line does, and the system behind the
//! gateway restarts whenever it is updated; this package is what makes those
//! breaks harmless. It keeps no product state: where an event ends up and what
//! a command does belong to whoever uses it.
//!
//! The pieces, from the wire upward:
//!
//! * [`framing`] and [`session`] are one connection: whole messages, three
//!   queues in which control goes before events and events before data, the
//!   handshake that proves who the machine is, and the liveness that gives a
//!   silent connection up. [`websocket`] puts a connection on a real socket,
//!   on either end, and [`outward`] reaches that socket directly or through
//!   the proxy a restricted unit's environment names.
//! * [`streams`] and [`spool`] carry events from a machine: numbered where
//!   they are sent, kept on disk until the system says it has them, repeated
//!   after any break, and given up in a known order when the disk is full.
//! * [`commands`] and [`journal`] carry commands to a machine: written down
//!   before they are acknowledged, run once however often they are sent,
//!   serial by key, and answered from the journal when they are repeated.
//! * [`tunnels`] are ordered byte streams between two ends with credit, a
//!   replay buffer and resumption; [`tunnels::relay`] is what a gateway does
//!   with them and [`tunnels::terminal`] is the end of one at the system.
//! * [`content`] moves large files in pieces the line carries, and goes on
//!   from where it stopped.
//! * [`reconnect`] orders the endpoints and spaces the attempts.
//!
//! Two handles put them together: [`machine::MachineLink`] is all a machine
//! needs of its link, and [`controller::ControllerLink`] is the system's end
//! of one machine's link.
//!
//! Wall time is read from the [`Clock`](orch_types::Clock) a caller gives;
//! durations are measured on the runtime's monotonic clock, so that a test can
//! pause it. Nothing here chooses a socket: a connection is whatever carries
//! whole messages, so that the same code runs on a real socket and on the
//! doubles of the test kit.

#![forbid(unsafe_code)]

pub mod commands;
pub mod content;
pub mod controller;
pub mod framing;
pub mod journal;
pub mod machine;
pub mod outward;
pub mod ports;
pub mod reconnect;
pub mod session;
pub mod spool;
pub mod streams;
pub mod tunnels;
pub mod websocket;
