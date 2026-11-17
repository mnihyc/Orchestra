//! Original native runtime sources, separate from permissions and requested lifecycle state.

use crate::{Conn, DbError};
use orch_link_contract::native_target::RuntimeSource;
use orch_types::Id;

/// Whether this source may describe the current physical start. The caller holds its
/// provider/environment lock first; this query then serializes the runtime's source facts.
/// Seen superseded sources and closed starts cannot become current again. An untracked
/// observation cannot confirm the state of a tracked start. This grants no operation.
pub async fn observe(
    conn: &mut Conn,
    host: Id,
    runtime: Id,
    source: RuntimeSource,
    terminal: bool,
) -> Result<bool, DbError> {
    let current: Option<(Id, Option<Id>)> =
        sqlx::query_as("SELECT host_id,native_incarnation FROM runtimes WHERE id=$1 FOR UPDATE")
            .bind(runtime)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((owner, current)) = current.filter(|(owner, _)| *owner == host) else {
        return Ok(false);
    };
    let Some(incarnation) = source.incarnation else {
        return Ok(current.is_none());
    };
    if incarnation.is_nil() {
        return Ok(false);
    }
    let inserted = sqlx::query(
        "INSERT INTO runtime_incarnations(id,runtime_id,host_id,closed) VALUES($1,$2,$3,$4)
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(incarnation)
    .bind(runtime)
    .bind(owner)
    .bind(terminal)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        > 0;
    // Read ownership without taking another runtime's source lock: a substituted UUID
    // must not make two independent runtime controls wait for one another's row locks.
    let (source_runtime, source_host, closed): (Id, Id, bool) =
        sqlx::query_as("SELECT runtime_id,host_id,closed FROM runtime_incarnations WHERE id=$1")
            .bind(incarnation)
            .fetch_one(&mut *conn)
            .await?;
    if source_runtime != runtime || source_host != host {
        return Ok(false);
    }
    if !inserted && (current != Some(incarnation) || (closed && !terminal)) {
        return Ok(false);
    }
    if inserted {
        if let Some(previous) = current {
            sqlx::query("UPDATE runtime_incarnations SET closed=true WHERE id=$1 AND runtime_id=$2 AND host_id=$3")
                .bind(previous).bind(runtime).bind(host).execute(&mut *conn).await?;
        }
        sqlx::query("UPDATE runtimes SET native_incarnation=$2 WHERE id=$1")
            .bind(runtime)
            .bind(incarnation)
            .execute(&mut *conn)
            .await?;
    } else if terminal && !closed {
        sqlx::query("UPDATE runtime_incarnations SET closed=true WHERE id=$1 AND runtime_id=$2 AND host_id=$3")
            .bind(incarnation).bind(runtime).bind(host).execute(&mut *conn).await?;
    }
    Ok(true)
}
