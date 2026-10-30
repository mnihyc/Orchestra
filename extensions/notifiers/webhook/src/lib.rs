//! Signed webhook notification delivery.
//!
//! The notifier of an address that is posted to: for a person, a webhook of
//! their own or of their org's, and for a node, a place that is told what
//! happens there. Each notification is posted as JSON, or, where the channel
//! or the destination has a text of its own, as what that renders; with a
//! secret the post is signed, so that the receiver knows it came from this
//! system and was not changed on the way.
//!
//! What an org or a person set up is held to the list of private hosts that
//! a system admin named: an address on a loopback, link-local or private
//! network is refused unless it is on the list, on the address a connection
//! goes to and again after every redirect.

#![forbid(unsafe_code)]

pub mod delivery;
pub mod signature;
pub mod targets;

pub use delivery::{NAME, WebhookNotifier, descriptor};
