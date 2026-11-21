//! Server-recorded execution decisions, separate from native observations and permissions.

use crate::{Conn, DbError};
use orch_agent_programs::confirmation::Confirmation;
use orch_api_contract::machine_programs::MachineExecutionSubject;
use orch_types::{Id, Json, Time};

/// A manager's factual decision, retained after its completed command is cleaned up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// The exact program selected by that manager.
    pub subject: MachineExecutionSubject,
    /// The server-recorded manager and moment.
    pub confirmation: Confirmation,
}

#[derive(sqlx::FromRow)]
struct Row {
    machine_id: Id,
    subject: Json,
    confirmed_by: Id,
    confirmed_at: chrono::DateTime<chrono::Utc>,
}

fn invalid() -> DbError {
    DbError::Rule {
        constraint: "machine_execution_confirmation".into(),
    }
}

/// Factual receipts for visible physical machines, in one indexed read.
pub async fn among(
    conn: &mut Conn,
    machines: &[Id],
) -> Result<std::collections::BTreeMap<Id, Vec<Decision>>, DbError> {
    let mut result = std::collections::BTreeMap::<Id, Vec<Decision>>::new();
    if machines.is_empty() {
        return Ok(result);
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT machine_id,subject,confirmed_by,confirmed_at FROM machine_execution_confirmations
         WHERE machine_id=ANY($1) ORDER BY id",
    )
    .bind(machines)
    .fetch_all(conn)
    .await?;
    for row in rows {
        let decision = Decision {
            subject: serde_json::from_value(row.subject).map_err(|_| invalid())?,
            confirmation: Confirmation {
                by: row.confirmed_by,
                at: Time::new(row.confirmed_at),
            },
        };
        result.entry(row.machine_id).or_default().push(decision);
    }
    Ok(result)
}

/// Factual receipts for this physical machine; callers still use its ordinary Read gate.
pub async fn of_machine(conn: &mut Conn, machine: Id) -> Result<Vec<Decision>, DbError> {
    Ok(among(conn, &[machine])
        .await?
        .remove(&machine)
        .unwrap_or_default())
}

/// Retain only a validated successful selection under the existing machine transaction.
/// The caller supplies its already admitted manager; this function authorizes nothing.
pub async fn put(conn: &mut Conn, machine: Id, decision: &Decision) -> Result<(), DbError> {
    if !super::transaction_guard::in_transaction(conn)
        || machine.is_nil()
        || decision.confirmation.by.is_nil()
    {
        return Err(invalid());
    }
    let subject = serde_json::to_value(&decision.subject).map_err(|_| invalid())?;
    sqlx::query(
        "INSERT INTO machine_execution_confirmations(id,machine_id,subject,confirmed_by,confirmed_at)
         VALUES($1,$2,$3,$4,$5)",
    )
    .bind(orch_types::new_id())
    .bind(machine)
    .bind(subject)
    .bind(decision.confirmation.by)
    .bind(decision.confirmation.at.get())
    .execute(conn)
    .await?;
    Ok(())
}

/// Replace only this harness's factual attribution, never another harness's decision.
pub async fn clear_program(conn: &mut Conn, machine: Id, kind: &str) -> Result<(), DbError> {
    if !super::transaction_guard::in_transaction(conn) {
        return Err(invalid());
    }
    sqlx::query(
        "DELETE FROM machine_execution_confirmations WHERE machine_id=$1
         AND subject->>'kind'='program' AND subject->>'harness_kind'=$2",
    )
    .bind(machine)
    .bind(kind)
    .execute(conn)
    .await?;
    Ok(())
}

/// The complete program policy was acknowledged; other execution subject kinds stay intact.
pub async fn clear_programs(conn: &mut Conn, machine: Id) -> Result<(), DbError> {
    if !super::transaction_guard::in_transaction(conn) {
        return Err(invalid());
    }
    sqlx::query(
        "DELETE FROM machine_execution_confirmations WHERE machine_id=$1 AND subject->>'kind'='program'",
    )
    .bind(machine)
    .execute(conn)
    .await?;
    Ok(())
}

/// A new system decision replaces only the previous platform review.
pub async fn clear_system(conn: &mut Conn, machine: Id) -> Result<(), DbError> {
    if !super::transaction_guard::in_transaction(conn) {
        return Err(invalid());
    }
    sqlx::query("DELETE FROM machine_execution_confirmations WHERE machine_id=$1 AND subject->>'kind'='system'")
        .bind(machine).execute(conn).await?;
    Ok(())
}
