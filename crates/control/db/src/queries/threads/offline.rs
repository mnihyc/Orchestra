//! One durable offline episode belongs to one exact native turn, never to a thread's next turn.

use super::*;

/// Actual per-turn episode, with the effective setting frozen when the outage began.
#[derive(Clone, Debug, PartialEq, Eq, sqlx::FromRow)]
pub struct Episode {
    /// Exact native machine of the turn.
    pub machine: Id,
    /// First offline observation for this machine and turn.
    pub since: DateTime<Utc>,
    /// Configured seconds, with no implicit upper cap; None retains indefinite waiting.
    pub after_secs: Option<i64>,
}
impl Episode {
    /// Compare elapsed seconds without imposing a representable timestamp as a policy cap.
    pub fn expired(&self, now: DateTime<Utc>) -> bool {
        self.after_secs
            .is_some_and(|seconds| now >= self.since && (now - self.since).num_seconds() >= seconds)
    }
    /// An ordinary timestamp for presentation, absent if the chosen duration is beyond this format.
    pub fn at(&self) -> Option<DateTime<Utc>> {
        self.since
            .checked_add_signed(chrono::Duration::try_seconds(self.after_secs?)?)
    }
}
/// Load only an active exact turn's recorded observation.
pub async fn episode(conn: &mut Conn, turn: Id) -> Result<Option<Episode>, DbError> {
    Ok(sqlx::query_as("SELECT offline_machine_id AS machine,offline_since AS since,offline_after_secs AS after_secs FROM turns WHERE id=$1 AND state IN ('starting','running') AND offline_since IS NOT NULL").bind(turn).fetch_optional(conn).await?)
}
/// The caller owns the thread lock. Duplicate offline observations retain the original setting/time.
pub async fn begin(
    conn: &mut Conn,
    turn: Id,
    machine: Id,
    at: DateTime<Utc>,
    after_secs: Option<i64>,
) -> Result<bool, DbError> {
    let changed=sqlx::query("UPDATE turns SET offline_since=$3,offline_machine_id=$2,offline_after_secs=$4 WHERE id=$1 AND state IN ('starting','running') AND (offline_since IS NULL OR offline_machine_id IS DISTINCT FROM $2)").bind(turn).bind(machine).bind(at).bind(after_secs).execute(conn).await?;
    Ok(changed.rows_affected() == 1)
}
/// A real reconnect/event or terminal transition clears only this exact turn's episode.
pub async fn clear(conn: &mut Conn, turn: Id) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE turns SET offline_since=NULL,offline_machine_id=NULL,offline_after_secs=NULL WHERE id=$1 AND offline_since IS NOT NULL").bind(turn).execute(conn).await?.rows_affected()==1)
}
/// The existing supervisor selects due active turns in bounded batches, without a second timer queue.
pub async fn due(conn: &mut Conn, now: DateTime<Utc>, limit: i64) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT u.thread_id FROM turns u JOIN threads t ON t.current_turn_id=u.id WHERE u.state IN ('starting','running') AND u.offline_after_secs IS NOT NULL AND u.offline_since IS NOT NULL AND extract(epoch FROM ($1::timestamptz-u.offline_since)) >= u.offline_after_secs ORDER BY u.offline_since,u.id LIMIT $2").bind(now).bind(limit).fetch_all(conn).await?)
}
