//! A request held at its source thread's row is let go only once it has taken its snapshot, and a failure says which wait it was.
//!
//! A case locks the thread's row in a transaction of its own and sends the request. Other things of the same database
//! can wait for that transaction too (the engine's work on the thread, a write that refers to it), so a helper that
//! asks for any waiting backend can let the case change the source, or end the key, before the request has taken the
//! snapshot it compares with: the request then starts after the change and is rightly served. The request is
//! therefore followed by a recorder of its own, and the database is asked only once the request has finished its
//! first authorization, which is the snapshot every later check compares with, and has begun to fetch what it needs:
//! from there on, whatever it waits for, a change made under the lock lands after its first check and before its
//! first write. What waited on the case's transaction at that moment is kept for the failure message.
use super::*;
use crate::support::failure_trace::{FailureTrace, Recorder};
use orch_api_core::testing::RawReply;
use thread_history_scope::Proof;
use tokio::task::JoinHandle;

/// How often the database is asked while the request is on its way.
const POLL: Duration = Duration::from_millis(2);

/// The wait of one request on the source row, which the case's own transaction holds.
pub(super) struct SourceWait {
    mode: &'static str,
    operation: &'static str,
    trace: FailureTrace,
    waiting: Vec<String>,
    active: Vec<String>,
}

impl SourceWait {
    /// `operation` is the route of the request and `mode` the variant of the case that runs.
    pub(super) fn new(operation: &'static str, mode: &'static str) -> Self {
        let trace = FailureTrace::new("fork request at its source thread's row", rig::PATIENCE);
        trace.mark(format!("{operation}, mode {mode}: the case holds the row"));
        Self {
            mode,
            operation,
            trace,
            waiting: Vec::new(),
            active: Vec::new(),
        }
    }

    /// The recorder the request is sent under: `call.send_bytes().with_subscriber(wait.recorder())`.
    pub(super) fn recorder(&self) -> Recorder {
        self.trace.recorder()
    }

    /// Waits until the request has finished its first authorization and a statement waits for the transaction of the
    /// backend `blocker`.
    pub(super) async fn reached(
        &mut self,
        w: &Forks,
        blocker: i32,
        pending: &JoinHandle<RawReply>,
    ) -> Proof {
        let recorder = self.trace.recorder();
        let operation = self.operation;
        let mut last = String::from("the request had not finished its first authorization");
        let found = tokio::time::timeout(rig::PATIENCE, async {
            loop {
                if pending.is_finished() {
                    return Err(format!("the request answered before it waited; {last}"));
                }
                // The fetch of the initial phase follows the snapshot of the first authorization.
                if recorder.entered(operation, "initial.fetch") {
                    let waiting = waiting_on(w, blocker)
                        .await
                        .map_err(|error| error.to_string())?;
                    if !waiting.is_empty() {
                        return Ok(waiting);
                    }
                    last = String::from(
                        "the request is past its first authorization and nothing waits on the case yet",
                    );
                }
                tokio::time::sleep(POLL).await;
            }
        })
        .await;
        let waiting = match found {
            Ok(Ok(waiting)) => waiting,
            Ok(Err(why)) => return Err(why.into()),
            Err(_) => {
                return Err(format!(
                    "{} in mode {}: nothing waited on the case's transaction within {:?}; {last}",
                    self.operation,
                    self.mode,
                    rig::PATIENCE
                )
                .into());
            }
        };
        self.waiting = waiting
            .iter()
            .map(|(pid, query)| format!("backend {pid}: {query}"))
            .collect();
        self.active = active_beside(w, blocker)
            .await
            .unwrap_or_else(|error| vec![error.to_string()]);
        self.trace.mark(format!(
            "let go; waiting on the case: {:?}; other active backends: {:?}",
            self.waiting, self.active
        ));
        Ok(())
    }

    /// The answer of a request that should have been refused, with the mode and what waited when the case went on.
    pub(super) fn explain(&self, reply: &RawReply) -> String {
        format!(
            "{} in mode {} was answered {} {}; the case went on while {:?} waited on its transaction, and these other backends were active: {:?}",
            self.operation,
            self.mode,
            reply.status,
            String::from_utf8_lossy(&reply.raw),
            self.waiting,
            self.active
        )
    }

    /// The case went through: nothing is printed.
    pub(super) fn finish(&mut self) {
        self.trace.finish();
    }
}

/// The backends of this database that wait for a lock the transaction of `blocker` holds, with their statements.
async fn waiting_on(w: &Forks, blocker: i32) -> Result<Vec<(i32, String)>, sqlx::Error> {
    sqlx::query_as::<_, (i32, String)>(
        "SELECT pid, COALESCE(query, '') FROM pg_stat_activity
          WHERE datname = current_database() AND wait_event_type = 'Lock' AND $1 = ANY(pg_blocking_pids(pid))
          ORDER BY pid",
    )
    .bind(blocker)
    .fetch_all(w.s.rig.db.pool())
    .await
}

/// The other backends of this database that are doing something, with what each waits for and runs.
async fn active_beside(w: &Forks, blocker: i32) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (i32, String, String, String)>(
        "SELECT pid, COALESCE(state, ''), COALESCE(wait_event_type || ':' || wait_event, ''), left(COALESCE(query, ''), 200)
           FROM pg_stat_activity
          WHERE datname = current_database() AND pid <> pg_backend_pid() AND pid <> $1 AND state IS DISTINCT FROM 'idle'
          ORDER BY pid",
    )
    .bind(blocker)
    .fetch_all(w.s.rig.db.pool())
    .await?;
    Ok(rows
        .into_iter()
        .map(|(pid, state, wait, query)| format!("backend {pid} {state} {wait}: {query}"))
        .collect())
}
