//! Online hosts whose reported drivers can propose a template without allocating a unit.

use crate::{Conn, DbError};
use orch_types::{Id, Json};

/// An online node and its host's declared drivers.
#[derive(Debug, sqlx::FromRow)]
pub struct Candidate {
    /// The host's resource identifier, for current Use authority.
    pub host: Id,
    /// Its node connector.
    pub machine: Id,
    /// Driver names mapped to their descriptors.
    pub drivers: Json,
}

/// Reads only online, live nodes; no capacity is reserved by an import.
pub async fn candidates(conn: &mut Conn) -> Result<Vec<Candidate>, DbError> {
    Ok(sqlx::query_as(
        "SELECT h.id AS host,h.machine_id AS machine,h.drivers FROM hosts h
        JOIN machines m ON m.id=h.machine_id WHERE m.online AND m.deleted_at IS NULL ORDER BY h.id",
    )
    .fetch_all(conn)
    .await?)
}
