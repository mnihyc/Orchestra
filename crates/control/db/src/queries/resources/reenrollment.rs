//! Existing machines locked and reactivated with replacement certificates while retaining their registrations.

use super::{MachineRow, NextCertificate};
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// Reads a machine under a row lock shared by re-enrollment and session authorization.
pub async fn lock(conn: &mut Conn, machine: Id) -> Result<Option<MachineRow>, DbError> {
    let exists: Option<Id> = sqlx::query_scalar("SELECT id FROM machines WHERE id = $1 FOR UPDATE")
        .bind(machine)
        .fetch_optional(&mut *conn)
        .await?;
    if exists.is_none() {
        return Ok(None);
    }
    super::machine(conn, machine).await
}

/// Finds the host that this machine is, rather than the host a managed runtime runs on.
pub async fn host_of(conn: &mut Conn, machine: Id) -> Result<Option<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT id FROM hosts WHERE machine_id = $1")
            .bind(machine)
            .fetch_optional(conn)
            .await?,
    )
}

/// Reactivates the machine with a pending certificate, keeping a revoked old certificate revoked.
pub async fn activate(
    conn: &mut Conn,
    machine: &MachineRow,
    next: &NextCertificate,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    if machine.lost_at.is_some() {
        return Err(DbError::Rule {
            constraint: "machine_permanently_lost".into(),
        });
    }
    super::set_next_certificate(conn, machine.id, next, now).await?;
    if !machine.is_active() {
        // A revoked certificate cannot become usable again when the machine becomes active.
        super::promote_next_certificate(conn, machine.id, &next.fingerprint, now).await?;
    }
    sqlx::query("UPDATE machines SET status = 'active', updated_at = $2 WHERE id = $1")
        .bind(machine.id)
        .bind(now)
        .execute(conn)
        .await?;
    Ok(())
}
