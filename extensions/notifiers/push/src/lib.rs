//! Web Push notification delivery.
//!
//! The notifier of Web Push, the one channel whose keys are the system's. A
//! person's route through it reaches each browser of theirs that took push: the
//! content is encrypted to that browser's key (RFC 8291), signed with the
//! system's VAPID key (RFC 8292), and posted to the endpoint its push service
//! gave the browser. The content is what the Inbox row shows: a title, a first
//! line and a link.
//!
//! An encryption-only library seals the message; P-256 signs the VAPID token. The post
//! uses the same kind of client as every other notifier's, to an address that has been looked
//! at, since a browser's endpoint is an address a person reported and must not
//! lead to a machine behind the system.
//!
//! A push service that says the subscription no longer exists makes the
//! browser's subscription go; one that is slow or down is tried again.

#![forbid(unsafe_code)]

pub mod delivery;
mod encryption;
pub mod payload;
pub mod vapid;

pub use delivery::{NAME, PushNotifier, descriptor};
pub use vapid::{Vapid, VapidError};
