//! A small world on a real database for the tests of the worker: people, channels, routes and notifications, a clock that stands still, notifiers that answer as they are told to, and a real mail server in Docker.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

pub mod fakes;
#[path = "../../../../../extensions/notifiers/email/tests/support/mailpit.rs"]
pub mod mailpit;
pub mod world;
