//! Permission catalogue, roles and permission-page contracts, without access decisions.
//!
//! Access is stated on the thing, permission by permission, as "who". This
//! crate holds what everybody who reads or writes that statement shares: the
//! closed catalogue of [permissions], the four [roles] and the hierarchy they
//! are held in, the shape of a permission [page](pages) and of its defaults,
//! what a [decision](decisions) and a refusal are, and the catalogue of
//! [audit] actions. It holds the rules that need nothing stored: how levels
//! nest, what a row comes to through the rows it follows, what a lock
//! decides, whether a change can be a change of a page at all, and how a
//! person holds a permission once it is known whom the rows name.
//!
//! It reads no storage and decides nothing about a stored thing. Whoever
//! does reads the rows and asks here.

#![forbid(unsafe_code)]

pub mod audit;
pub mod decisions;
pub mod pages;
pub mod permissions;
pub mod roles;

pub use audit::{ActorKind, AuditAction};
pub use decisions::{Decision, MissingAccess, Refusal, Standing, Through};
pub use pages::{Chain, Entry, Holder, PageChange, PageOf, Row};
pub use permissions::{Choice, Level, ObjectKind, Permission, Placement, Profile, ResourceKind};
pub use roles::{NodeKind, PlaceKind, PlaceRef, Role, Subject};
