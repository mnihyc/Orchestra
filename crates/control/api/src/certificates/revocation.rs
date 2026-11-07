//! The native registration withdrawal owns its row and command cancellation footprint.

use super::{Actor, CertificateService, DateTime, DbError, Id, Origin, Utc};
use orch_db::Conn;

/// Factual native operands, usable only in the original transaction. This is not authority.
pub(crate) struct Revocation {
    machine: Id,
    active: bool,
    transaction: String,
}

fn another_transaction() -> DbError {
    DbError::Rule {
        constraint: "machine_revocation_transaction".to_owned(),
    }
}

impl CertificateService {
    /// Hold the physical registration and every command the native writer will cancel.
    /// Holding the physical row also serializes new command foreign-key references.
    pub(crate) async fn prepare_revocation(
        conn: &mut Conn,
        machine: Id,
    ) -> Result<Revocation, DbError> {
        if !orch_db::queries::transaction_guard::in_transaction(conn) {
            return Err(another_transaction());
        }
        let status: Option<String> =
            sqlx::query_scalar("SELECT status FROM machines WHERE id=$1 FOR UPDATE")
                .bind(machine)
                .fetch_optional(&mut *conn)
                .await?;
        // A Host facade is the permission subject of a node-role Machine.
        sqlx::query("SELECT id FROM hosts WHERE machine_id=$1 ORDER BY id FOR SHARE")
            .bind(machine)
            .fetch_all(&mut *conn)
            .await?;
        let active = status.as_deref() == Some("active");
        if active {
            sqlx::query("SELECT id FROM commands WHERE machine_id=$1 AND state IN ('queued','sent','accepted') ORDER BY id FOR UPDATE")
                .bind(machine)
                .fetch_all(&mut *conn)
                .await?;
        }
        let transaction = sqlx::query_scalar("SELECT pg_current_xact_id()::text")
            .fetch_one(&mut *conn)
            .await?;
        Ok(Revocation {
            machine,
            active,
            transaction,
        })
    }

    /// Consume those same operands, invoking the once-only cut at the first native UPDATE.
    pub(crate) async fn apply_revocation<E, F>(
        conn: &mut Conn,
        prepared: Revocation,
        by: Actor,
        origin: &Origin,
        now: DateTime<Utc>,
        before: F,
    ) -> Result<Result<bool, DbError>, E>
    where
        F: FnOnce() -> Result<(), E>,
    {
        let valid: Result<(), DbError> = async {
            if !orch_db::queries::transaction_guard::in_transaction(conn) {
                return Err(another_transaction());
            }
            let current: String = sqlx::query_scalar("SELECT pg_current_xact_id()::text")
                .fetch_one(&mut *conn)
                .await?;
            if current != prepared.transaction {
                return Err(another_transaction());
            }
            Ok(())
        }
        .await;
        if let Err(error) = valid {
            return Ok(Err(error));
        }
        if !prepared.active {
            return Ok(Ok(false));
        }
        before()?;
        Ok(Self::write_revocation(conn, prepared.machine, by, origin, now).await)
    }
}

#[cfg(test)]
mod tests;
