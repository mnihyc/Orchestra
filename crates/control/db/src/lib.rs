//! PostgreSQL persistence, transactions and projections.
//!
//! Everything the control plane keeps is in one PostgreSQL database. This
//! package is the only one that writes SQL. A query is a function that
//! takes a connection, so that the same function runs alone or inside a
//! transaction that its caller holds; a rule that spans rows is kept by
//! running its statements in one transaction, and a change of state is a
//! compare-and-set, never a read followed by a write.

#![forbid(unsafe_code)]

pub mod audit;
pub mod heartbeat;
pub mod idempotency;
pub mod migrations;
pub mod pool;
pub mod programs;
pub mod queries;
pub mod reconciliation;
pub mod secrets;
pub mod seeds;
pub mod sequences;
pub mod usage;

pub use pool::{Conn, Db, DbError, Tx};
