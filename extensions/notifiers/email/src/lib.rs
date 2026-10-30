//! SMTP notification delivery across configured security and authentication modes.
//!
//! The notifier of a mail server. A channel of the kind `email` is any
//! server that speaks SMTP submission: with TLS from the first byte, with
//! STARTTLS or without encryption; signed in with `PLAIN`, `LOGIN` or
//! whichever the server offers, or not at all; with a certificate that a
//! public authority signed or one that the channel names an authority for.
//! It sends a finished text, as plain text and as HTML made from it, to the
//! confirmed address of one person; it is also what carries the mail that is
//! no notification, such as a confirmation or a code to sign in with.
//!
//! A server is looked at when its channel is saved ([`smtp::check`]): it is
//! connected to, the connection is secured, and the client signs in. What the
//! server refuses for good refuses the save, with its own words; a server
//! that cannot be reached now does not, since a server that is down today is
//! not a wrong entry.

#![forbid(unsafe_code)]

pub mod delivery;
pub mod render;
pub mod smtp;

pub use delivery::{EmailNotifier, NAME, descriptor};
