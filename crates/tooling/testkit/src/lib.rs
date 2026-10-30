//! Behavioral doubles, a clock that moves when told and a database of its own for each test, for the tests of the other packages.
//!
//! What tests of the other packages stand on: a database of their own, a
//! clock that moves when told, and doubles for the other side of each
//! seam.

#![forbid(unsafe_code)]

pub mod clock;
pub mod database;
pub mod doubles;
pub mod fixtures;
pub mod native_configuration;
pub mod oidc;
pub mod prefix_history;
pub mod programs;
pub mod slow_link;
pub mod workflows;
